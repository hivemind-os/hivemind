# Config Schemas

Plugins define their configuration using Zod schemas with Hivemind UI extensions. The host serializes these schemas and renders them as forms in the Settings UI.

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
