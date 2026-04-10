# Troubleshooting

## License Issues

### "Pro feature requires a license"

```bash
# Check your license status
wshm login --license

# Or set via environment
export WSHM_LICENSE_KEY=WSHM-XXXX-XXXX-XXXX
```

### "License validation failed"

- Check your internet connection (first activation requires API call)
- Verify the key is correct (starts with `WSHM-`)
- Check if the license has expired
- Check if you've exceeded max activations (default: 3 machines)

### "Cannot reach license server"

wshm validates licenses offline after initial activation. If `~/.wshm/license.jwt` exists and hasn't expired, no network call is needed.

```bash
# Check if JWT exists
cat ~/.wshm/license.jwt | head -c 50
```

### Vault placeholder not resolving

1. Check vault config is present in `.wshm/config.toml`:
   ```toml
   [vault]
   provider = "hashicorp"
   address = "https://vault.company.com"
   ```
2. Check auth environment variables are set (e.g. `VAULT_TOKEN`)
3. Test vault access directly:
   ```bash
   vault kv get secret/wshm
   ```
4. Check wshm was compiled with the right feature flag:
   ```bash
   wshm --version  # should show vault-hashicorp in features
   ```

## GitHub Issues

### "Rate limit exceeded"

wshm uses ETags and conditional requests to minimize API calls. If you hit rate limits:

```bash
# Check remaining rate limit
curl -H "Authorization: Bearer $GITHUB_TOKEN" https://api.github.com/rate_limit

# Use offline mode while waiting
wshm run --offline
```

### "No issues/PRs found"

```bash
# Force a full sync
wshm sync

# Check the cache
wshm --json | jq
```

### "Authentication error"

```bash
# Verify your token
echo $GITHUB_TOKEN

# Or login interactively
wshm login
```

## AI Provider Issues

### "AI request failed"

```bash
# Check your API key is set
echo $ANTHROPIC_API_KEY

# Test with verbose output
wshm triage --issue 1 -v
```

### "Timeout on AI call"

Large PRs or complex issues may take longer. The default timeout is 60 seconds. For very large diffs, wshm splits the review per-file.

### Using a different provider

```toml
[ai]
provider = "openai"
model = "gpt-4o"
# Or local:
# provider = "ollama"
# model = "llama3"
# base_url = "http://localhost:11434"
```

## Database Issues

### "Database is locked"

SQLite only allows one writer at a time. If you're running multiple wshm processes:

```bash
# Use PostgreSQL for concurrent access
[database]
provider = "postgresql"
uri = "postgres://user:pass@localhost/wshm"
```

### Corrupted state.db

```bash
# Delete and re-sync
rm .wshm/state.db
wshm sync
```

## Daemon Issues

### "Address already in use"

```bash
# Check what's using the port
lsof -i :3000

# Use a different port
[daemon]
bind = "127.0.0.1:3001"
```

### Webhooks not received

1. Check the webhook URL is publicly accessible
2. Verify the webhook secret matches:
   ```toml
   [daemon]
   webhook_secret = "same-as-github"
   ```
3. Check GitHub webhook delivery log: Repo Settings > Webhooks > Recent Deliveries
4. Check daemon logs:
   ```bash
   journalctl -u wshm -f
   ```

### Web dashboard shows "Unauthorized"

```toml
[web]
username = "admin"
password = "your-password"
```

If you forgot the password, check `.wshm/credentials` or remove the `[web]` password line to disable auth.

## Common Errors

| Error | Cause | Fix |
|-------|-------|-----|
| `No .wshm/config.toml found` | Not initialized | `wshm config init` |
| `GITHUB_TOKEN not set` | Missing token | `export GITHUB_TOKEN=ghp_xxx` |
| `Pro feature: review` | No license | `wshm login --license` |
| `Sync failed: 401` | Invalid GitHub token | Check/regenerate token |
| `AI error: 429` | Rate limited | Wait or switch provider |
| `Database locked` | Concurrent access | Use PostgreSQL |

## Getting Help

- GitHub Issues: https://github.com/wshm-dev/wshm/issues
- Documentation: https://wshm.dev/docs
- Email: contact@wshm.dev
