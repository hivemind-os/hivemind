# Config Schemas

Plugins define their configuration using Zod schemas with Hivemind UI extensions. The SDK extracts these schemas at build time into a static `dist/config-schema.json` file, which the host reads to render config forms — no running plugin process needed.

## How It Works

1. You define a `configSchema` using Zod in your plugin's `definePlugin()` call
2. When you run `npm run build`, `tsc` compiles your code, then `hivemind-extract-schema` imports the built plugin and serializes the Zod schema to `dist/config-schema.json`
3. When the plugin is installed/registered, Hivemind reads `dist/config-schema.json` and renders the config form in Settings → Plugins

> **Important:** Always run the full build (`tsc && hivemind-extract-schema`) after changing your config schema. The config form is driven by `dist/config-schema.json`, not the live plugin process.

## Basic Schema

```typescript
import { z } from '@hivemind/plugin-sdk';

configSchema: z.object({
  apiKey: z.string(),
  maxResults: z.number().default(20),
  enabled: z.boolean().default(true),
})
```

## UI Extensions

### Labels and Help Text

```typescript
z.string()
  .label('API Key')                    // field label in the form
  .helpText('Find this in Settings')   // tooltip text
  .placeholder('sk-...')               // input placeholder
```

### Sections

Group related fields into collapsible sections:

```typescript
z.object({
  apiKey: z.string().section('Authentication'),
  clientId: z.string().section('Authentication'),

  owner: z.string().section('Repository'),
  repo: z.string().section('Repository'),

  pollInterval: z.number().section('Advanced'),
})
```

### Secret Fields

Fields marked `.secret()` are:
- Rendered as password inputs (masked)
- Stored in the OS keyring, not in config files
- Never logged or serialized to disk

```typescript
z.string().secret().label('API Key')
```

### Enum Rendering

```typescript
// Dropdown (default)
z.enum(['fast', 'balanced', 'thorough'])

// Radio buttons
z.enum(['fast', 'balanced', 'thorough']).radio()
```

## Supported Types

| Zod Type | UI Control | Notes |
|----------|-----------|-------|
| `z.string()` | Text input | |
| `z.string().secret()` | Password input | Stored in keyring |
| `z.number()` | Number input | Supports `.min()`, `.max()` |
| `z.boolean()` | Checkbox | |
| `z.enum([...])` | Dropdown | Use `.radio()` for radio buttons |
| `z.array(z.string())` | Tag/list input | |
| `z.string().optional()` | Optional text input | |
| `z.number().default(N)` | Number with default | |

## Validation

Config is validated against the schema when the user saves settings:

```typescript
// The host calls plugin/validateConfig automatically.
// You can also validate in onActivate for runtime checks:
onActivate: async (ctx) => {
  if (!ctx.config.apiKey.startsWith('sk-')) {
    throw new Error('API key must start with "sk-"');
  }
}
```

## Serialization Format

The host serializes your Zod schema to JSON for rendering:

```json
{
  "type": "object",
  "properties": {
    "apiKey": {
      "type": "string",
      "hivemind": {
        "label": "API Key",
        "secret": true,
        "section": "Authentication"
      }
    },
    "maxResults": {
      "type": "number",
      "default": 20,
      "minimum": 1,
      "maximum": 100,
      "hivemind": {
        "label": "Max Results",
        "section": "Options"
      }
    }
  },
  "required": ["apiKey"]
}
```

You can inspect this programmatically:

```typescript
import { serializeConfigSchema } from '@hivemind/plugin-sdk';

const schema = serializeConfigSchema(myPlugin.configSchema);
console.log(JSON.stringify(schema, null, 2));
```
