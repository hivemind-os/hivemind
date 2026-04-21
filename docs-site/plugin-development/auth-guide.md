# Authentication Guide

Plugins can declare their auth requirements and the host will help guide users through setup.

## Token-Based Auth

The simplest approach — user provides an API key or token:

```typescript
export default definePlugin({
  configSchema: z.object({
    token: z.string().secret().label('API Token').section('Authentication'),
    // ... other config
  }),

  auth: {
    type: 'token',
    fields: [
      {
        key: 'token',
        label: 'API Token',
        helpUrl: 'https://myservice.com/settings/tokens',
        helpText: 'Create a token with read/write permissions',
      },
    ],
  },

  onActivate: async (ctx) => {
    // Validate the token
    const res = await fetch('https://api.myservice.com/me', {
      headers: { Authorization: `Bearer ${ctx.config.token}` },
    });
    if (!res.ok) {
      throw new Error('Invalid API token');
    }
  },
});
```

## OAuth2 Auth

For services that use OAuth2:

```typescript
auth: {
  type: 'oauth2',
  authorizationUrl: 'https://myservice.com/oauth/authorize',
  tokenUrl: 'https://myservice.com/oauth/token',
  scopes: ['read', 'write'],
  pkce: true,           // recommended for desktop apps
  clientId: 'my-app',   // optional — can be set in config instead
},
```

The host handles the OAuth flow:
1. Opens the authorization URL in the user's browser
2. Listens for the callback
3. Exchanges the code for tokens
4. Stores tokens in the OS keyring

Your plugin accesses the token via secrets:

```typescript
onActivate: async (ctx) => {
  const accessToken = await ctx.secrets.get('oauth_access_token');
  if (!accessToken) {
    throw new Error('Not authenticated. Complete OAuth setup first.');
  }
},
```

## Secret Storage

Use `ctx.secrets` for any sensitive data:

```typescript
// Store a secret
await ctx.secrets.set('refresh_token', newToken);

// Read a secret
const token = await ctx.secrets.get('access_token');

// Check existence
if (await ctx.secrets.has('api_key')) { ... }

// Delete
await ctx.secrets.delete('old_token');
```

Secrets are:
- Stored in the OS keyring (macOS Keychain, Windows Credential Manager, Linux Secret Service)
- Scoped to your plugin — you can't read other plugins' secrets
- Never written to config files or logs

## Token Refresh

For OAuth2, handle token refresh in your tools or loop:

```typescript
async function getValidToken(ctx: PluginContext): Promise<string> {
  let token = await ctx.secrets.get('access_token');
  const expiry = await ctx.store.get<number>('token_expiry');

  if (!token || (expiry && Date.now() > expiry)) {
    const refreshToken = await ctx.secrets.get('refresh_token');
    if (!refreshToken) throw new Error('No refresh token — re-authenticate');

    const result = await refreshOAuthToken(refreshToken);
    await ctx.secrets.set('access_token', result.access_token);
    await ctx.secrets.set('refresh_token', result.refresh_token);
    await ctx.store.set('token_expiry', Date.now() + result.expires_in * 1000);

    token = result.access_token;
  }

  return token;
}
```
