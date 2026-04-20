# Plugin Registry

This directory contains the plugin registry index for Hivemind plugin discovery.

## Structure

- `registry.json` — The main registry index containing all known plugins and categories

## How it works

The desktop app fetches `registry.json` from a known URL (GitHub raw content or CDN) and renders it in the Plugin Browser UI. Plugin authors submit their plugins by opening a PR that adds an entry to the `plugins` array in `registry.json`.

## Submitting a Plugin

1. Publish your plugin to npm
2. Fork this repository
3. Add your plugin entry to `registry.json`
4. Open a PR with your addition

### Plugin Entry Format

```json
{
  "name": "@your-scope/hivemind-connector-name",
  "displayName": "Human Readable Name",
  "description": "What the plugin does",
  "npmPackage": "@your-scope/hivemind-connector-name",
  "categories": ["category-id"],
  "verified": false,
  "featured": false,
  "icon": "https://your-cdn.com/icon.svg",
  "author": "your-github-username",
  "repository": "https://github.com/you/repo",
  "license": "MIT",
  "pluginType": "connector",
  "services": ["what-services"],
  "permissions": ["secrets:read"],
  "minHostVersion": "0.2.0"
}
```

## Verification

Plugins with `"verified": true` have been reviewed by Hivemind maintainers for:
- Code quality and security
- Correct use of the plugin SDK
- No malicious behavior
- Accurate description and metadata
