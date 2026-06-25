# Docker Usage

## Building the Docker Image

The Docker image is built using Nix to ensure reproducibility:

```bash
# Build the Docker image
nix build .#docker

# Load the image into Docker
docker load < result

# Verify the image is loaded
docker images | grep nanduti
```

## Running with Docker

### Basic Usage

```bash
# Run the server
docker run -d \
  --name nanduti \
  -p 3517:3517 \
  -v $(pwd)/data:/data \
  -e RUST_LOG=info \
  nanduti:latest \
  server --port 3517 --data-dir /data \
  --nostr-relay wss://relay.damus.io
```

### Using Docker Compose

```bash
# Start the service
docker-compose up -d

# View logs
docker-compose logs -f

# Stop the service
docker-compose down
```

## Volume Mounts

The container expects a `/data` volume for persistent storage:
- Federation data
- Mnemonic files
- NWC connection state

## Environment Variables

- `RUST_LOG`: Log level (e.g., `info`, `debug`, `trace`)
- `SSL_CERT_FILE`: CA certificates path (set automatically)

## CLI Commands in Docker

### Join a Federation

```bash
docker exec nanduti nanduti federation join \
  --invite-code "fed1..." \
  --data-dir /data
```

### List Federations

```bash
docker exec nanduti nanduti federation list \
  --data-dir /data
```

### Check Balance

```bash
docker exec nanduti nanduti balance \
  --federation-id <federation-id> \
  --data-dir /data
```

### Create Invoice

```bash
docker exec nanduti nanduti invoice \
  --federation-id <federation-id> \
  --amount 1000 \
  --description "Payment" \
  --data-dir /data
```

## Multi-Architecture Support

The Nix flake can build images for different architectures:

```bash
# Build for ARM64
nix build .#docker --system aarch64-linux

# Build for x86_64 (default)
nix build .#docker --system x86_64-linux
```

## Security Considerations

1. **Data Directory**: Always mount a persistent volume for `/data` to avoid losing federation data
2. **Network**: Only expose port 3517 if you need external access
3. **Secrets**: Store mnemonics securely - the container uses encrypted storage
4. **TLS**: Use a reverse proxy (nginx, traefik) for TLS termination in production

## Troubleshooting

### View Container Logs

```bash
docker logs nanduti
```

### Debug Mode

```bash
docker run -it --rm \
  -e RUST_LOG=debug \
  nanduti:latest \
  --help
```

### Shell Access

```bash
docker run -it --rm \
  --entrypoint /bin/bash \
  nanduti:latest
```