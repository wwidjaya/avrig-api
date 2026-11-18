.PHONY: build build-fe build-windows build-linux run stop status run-windows stop-windows status-windows deploy deploy-windows deploy-linux release-patch release-minor release-major release

build: build-fe
ifeq ($(OS),Windows_NT)
	@echo "Building Windows release binary..."
	cargo build --release
	@echo "Building Linux release binary via WSL..."
	wsl.exe -d avrig-ai -- bash -lc "source ~/.cargo/env 2>/dev/null || source /home/avrig/.cargo/env 2>/dev/null; cd /mnt/d/Cloud/OneDrive/AI/Agentic/avrig-api && cargo build --release"
else
	@echo "Building Linux release binary..."
	cargo build --release
	@echo "Building Windows release binary via cross-compilation target x86_64-pc-windows-msvc / gnu..."
	cargo build --release --target x86_64-pc-windows-gnu 2>/dev/null || cargo build --release --target x86_64-pc-windows-msvc 2>/dev/null || echo "Note: Windows target requires appropriate toolchain"
endif

build-fe:
	@echo "Building frontend web distribution..."
	cd ../avrig-ai && pnpm --filter @avrig/web build

build-windows: build-fe
	cargo build --release

build-linux: build-fe
	wsl.exe -d avrig-ai -- bash -lc "source ~/.cargo/env 2>/dev/null || source /home/avrig/.cargo/env 2>/dev/null; cd /mnt/d/Cloud/OneDrive/AI/Agentic/avrig-api && cargo build --release"

run:
	wsl.exe -d avrig-ai -- bash -lc "cd /mnt/d/Cloud/OneDrive/AI/Agentic/avrig-api && sh scripts/run.sh"

stop:
	wsl.exe -d avrig-ai -- bash -lc "cd /mnt/d/Cloud/OneDrive/AI/Agentic/avrig-api && sh scripts/stop.sh"

status:
	wsl.exe -d avrig-ai -- bash -lc "cd /mnt/d/Cloud/OneDrive/AI/Agentic/avrig-api && sh scripts/status.sh"

run-windows:
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/run.ps1

stop-windows:
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/stop.ps1

status-windows:
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/status.ps1

deploy:
ifeq ($(OS),Windows_NT)
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/deploy.ps1
else
	@echo "make deploy requires Windows host environment to coordinate Windows and WSL targets." >&2
	@exit 1
endif

deploy-windows:
ifeq ($(OS),Windows_NT)
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/deploy.ps1 -Target windows
else
	@echo "deploy-windows requires Windows host environment." >&2
	@exit 1
endif

deploy-linux:
ifeq ($(OS),Windows_NT)
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/deploy.ps1 -Target linux
else
	@echo "deploy-linux requires Windows host environment to coordinate WSL targets." >&2
	@exit 1
endif

# Release targets. The canonical release logic lives in avrig-ai/scripts/release.ps1
# because it also owns the workspace version and the embedded web package version.
release-patch:
ifeq ($(OS),Windows_NT)
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File ../avrig-ai/scripts/release.ps1 patch
else
	@pwsh -NoProfile -File ../avrig-ai/scripts/release.ps1 patch
endif

release-minor:
ifeq ($(OS),Windows_NT)
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File ../avrig-ai/scripts/release.ps1 minor
else
	@pwsh -NoProfile -File ../avrig-ai/scripts/release.ps1 minor
endif

release-major:
ifeq ($(OS),Windows_NT)
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File ../avrig-ai/scripts/release.ps1 major
else
	@pwsh -NoProfile -File ../avrig-ai/scripts/release.ps1 major
endif

release:
ifeq ($(OS),Windows_NT)
	powershell.exe -NoProfile -ExecutionPolicy Bypass -File ../avrig-ai/scripts/release.ps1 $(VERSION)
else
	@pwsh -NoProfile -File ../avrig-ai/scripts/release.ps1 $(VERSION)
endif
