.PHONY: example down logs test parity docs docs-dev

# Run the medieval example (bus, servers, client) in Docker. Open http://localhost:5173
example:
	docker compose -f examples/medieval/docker-compose.yml up --build

down:
	docker compose -f examples/medieval/docker-compose.yml down

logs:
	docker compose -f examples/medieval/docker-compose.yml logs -f

# Everything that runs without Docker: Rust tests, TypeScript tests and type checks.
test:
	cargo test --workspace
	npm install --no-audit --no-fund
	npm test --workspaces --if-present
	npm run typecheck

# The example client's TypeScript movement against the Rust original, bit for bit.
parity:
	npm run parity -w medieval-client

# The documentation site (docs/): build it, or serve it while editing.
docs:
	npm install --no-audit --no-fund --prefix docs && npm run build --prefix docs

docs-dev:
	npm install --no-audit --no-fund --prefix docs && npm run dev --prefix docs
