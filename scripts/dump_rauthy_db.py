import sqlite3

con = sqlite3.connect('/var/lib/rauthy/data/state_machine/db/hiqlite.db')
tables = [r[0] for r in con.execute("SELECT name FROM sqlite_master WHERE type='table';").fetchall()]
print("Tables:", tables)

print("\n--- Schema of clients ---")
for r in con.execute("PRAGMA table_info(clients);").fetchall():
    print(r)

print("\n--- Existing Clients ---")
for r in con.execute("SELECT * FROM clients;").fetchall():
    print(r)

print("\n--- Existing Scopes ---")
for r in con.execute("PRAGMA table_info(scopes);").fetchall():
    print(r)
for r in con.execute("SELECT * FROM scopes;").fetchall():
    print(r)
