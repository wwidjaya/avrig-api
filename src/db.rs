use crate::chunker::DocumentChunk;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;
use tokio_postgres::{Client, NoTls};
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "postgres://avrig-ai:h5iIo6p9QpK07h853QzHZWPA82aYTHy4@postgresql.avrig.ai:5432/avrig-db".into(),
        }
    }
}

pub struct DbRepository {
    url: String,
}

impl DbRepository {
    pub fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
        }
    }

    async fn connect(&self) -> Result<Client> {
        let (client, connection) = tokio_postgres::connect(&self.url, NoTls)
            .await
            .context("connecting to PostgreSQL avrig-db")?;

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::error!("PostgreSQL connection error: {}", e);
            }
        });

        Ok(client)
    }

    pub async fn check_document_by_hash(&self, sha256: &str) -> Result<Option<Uuid>> {
        let client = self.connect().await?;
        let row = client
            .query_opt(
                "SELECT id FROM km.document WHERE sha256 = $1 AND is_deleted = false LIMIT 1;",
                &[&sha256],
            )
            .await?;

        Ok(row.map(|r| r.get(0)))
    }

    pub async fn insert_document_with_vectors(
        &self,
        file_name: &str,
        sha256: &str,
        mime_type: &str,
        size_bytes: i64,
        embedding_model: &str,
        chunks: &[DocumentChunk],
        embeddings: &[Vec<f32>],
    ) -> Result<Uuid> {
        anyhow::ensure!(
            chunks.len() == embeddings.len(),
            "chunk and embedding counts differ"
        );
        let mut client = self.connect().await?;
        let transaction = client.transaction().await?;
        let doc_code = format!("DOC-{}", &sha256[..8].to_uppercase());
        let zero_uuid = Uuid::nil();

        let row = transaction
            .query_one(
                r#"
            INSERT INTO km.document (
                sha256, file_name, mime_type, size_bytes, storage_bucket, storage_key,
                document_code, document_name, document_desc,
                is_active, created_by, created_time, updated_by, updated_time,
                is_deleted, deleted_by, deleted_time, embedding_model
            ) VALUES (
                $1, $2, $3, $4, 'local-upload', $1,
                $5, $2, $2,
                true, $6, NOW(), $6, NOW(),
                false, $6, NOW(), $7
            )
            RETURNING id;
            "#,
                &[
                    &sha256,
                    &file_name,
                    &mime_type,
                    &size_bytes,
                    &doc_code,
                    &zero_uuid,
                    &embedding_model,
                ],
            )
            .await?;

        let doc_id: Uuid = row.get(0);
        let zero_uuid = Uuid::nil();

        for (chunk, emb) in chunks.iter().zip(embeddings.iter()) {
            // 32 UUID hex characters plus prefix remain within varchar(30).
            let unique = Uuid::new_v4().simple().to_string();
            let vec_code = format!("V{}", &unique[..29]);
            // Format embedding as pgvector literal string: [0.1,0.2,...]
            let emb_str = format!(
                "[{}]",
                emb.iter()
                    .map(|f| f.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            );

            let sql = format!(
                r#"
                INSERT INTO km.document_vector (
                    document_id, chunk_index, page_no, chunk_text, token_count,
                    embedding, tsv_content, start_char_offset, end_char_offset,
                    document_vector_code, document_vector_name, document_vector_desc,
                    is_active, created_by, created_time, updated_by, updated_time,
                    is_deleted, deleted_by, deleted_time
                ) VALUES (
                    $1, $2, $3, $4, $5,
                    '{emb_str}'::vector, to_tsvector('english', $4), $6, $7,
                    $8, $8, $8,
                    true, $9, NOW(), $9, NOW(),
                    false, $9, NOW()
                );
                "#
            );

            transaction
                .execute(
                    &sql,
                    &[
                        &doc_id,
                        &chunk.chunk_index,
                        &chunk.page_no,
                        &chunk.chunk_text,
                        &chunk.token_count,
                        &chunk.start_char_offset,
                        &chunk.end_char_offset,
                        &vec_code,
                        &zero_uuid,
                    ],
                )
                .await?;
        }

        transaction.commit().await?;
        Ok(doc_id)
    }

    pub async fn search_vectors(
        &self,
        query_embedding: &[f32],
        limit: i64,
    ) -> Result<Vec<SearchResult>> {
        let client = self.connect().await?;
        let emb_str = format!(
            "[{}]",
            query_embedding
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        let rows = client
            .query(
                r#"
                SELECT
                    dv.id, dv.document_id, dv.chunk_index, dv.page_no, dv.chunk_text,
                    (1.0 - (dv.embedding <=> $2::text::vector))::float4 AS vector_score,
                    d.document_code, d.document_name, d.file_name, d.mime_type, d.sha256
                FROM km.document_vector dv
                JOIN km.document d ON d.id = dv.document_id
                WHERE dv.is_deleted = false
                ORDER BY dv.embedding <=> $2::text::vector ASC
                LIMIT $1;
                "#,
                &[&limit, &emb_str],
            )
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| SearchResult {
                id: r.get("id"),
                document_id: r.get("document_id"),
                chunk_index: r.get("chunk_index"),
                page_no: r.get("page_no"),
                chunk_text: r.get("chunk_text"),
                vector_score: r.get("vector_score"),
                document_code: r
                    .get::<_, Option<String>>("document_code")
                    .unwrap_or_default(),
                document_name: r
                    .get::<_, Option<String>>("document_name")
                    .unwrap_or_default(),
                file_name: r.get("file_name"),
                mime_type: r.get("mime_type"),
                sha256: r.get("sha256"),
            })
            .collect())
    }
}

/// Nearest-neighbour passage lookup used by the search pipelines.
///
/// [`DbRepository`] implements this by delegating to
/// [`DbRepository::search_vectors`], so production behaviour lives in exactly
/// one place. The trait exists so the search handlers can be exercised end to
/// end without a live PostgreSQL instance: a test substitutes its own
/// implementation and the handlers keep running their real code path.
pub trait VectorIndex: Send + Sync {
    fn search_vectors<'a>(
        &'a self,
        query_embedding: &'a [f32],
        limit: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<SearchResult>>> + Send + 'a>>;
}

impl VectorIndex for DbRepository {
    fn search_vectors<'a>(
        &'a self,
        query_embedding: &'a [f32],
        limit: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<SearchResult>>> + Send + 'a>> {
        Box::pin(async move { DbRepository::search_vectors(self, query_embedding, limit).await })
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchResult {
    pub id: Uuid,
    pub document_id: Uuid,
    pub chunk_index: i32,
    pub page_no: Option<i32>,
    pub chunk_text: String,
    /// Cosine similarity from pgvector, in `-1.0..=1.0` for normalised embeddings.
    pub vector_score: f32,
    pub document_code: String,
    pub document_name: String,
    pub file_name: String,
    pub mime_type: String,
    pub sha256: String,
}
