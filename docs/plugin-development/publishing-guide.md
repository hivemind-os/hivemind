# Publishing Guide

How to publish your plugin and make it available to the Hivemind community.

## Publishing to npm

Hivemind plugins are standard npm packages. Publishing works the same as any npm package.

### 1. Prepare your package.json

Ensure you have:

```json
{
  "name": "@yourscope/hivemind-connector-myservice",
  "version": "1.0.0",
  "type": "module",
  "main": "dist/index.js",
  "files": ["dist", "assets", "README.md"],
  "hivemind": {
    "type": "connector",
    "displayName": "My Service",
    "description": "Connect Hivemind to My Service for task management",
    "categories": ["productivity", "project-management"],
    "permissions": ["network:api.myservice.com", "secrets:read", "loop:background"],
    "minHostVersion": "0.2.0"
  }
}
```

**Naming convention:** `hivemind-connector-<service>` or `@scope/hivemind-connector-<service>`

### 2. Build

```bash
npm run build  # runs: tsc && hivemind-extract-schema
```

This compiles your TypeScript and extracts `dist/config-schema.json`, which Hivemind uses to render your config form. Make sure `dist/config-schema.json` is included in your `"files"` array (it's inside `dist/` so it's covered by default).

### 3. Test

```bash
npm test
```

### 4. Publish

```bash
npm publish --access public
```

## Submitting to the Hivemind Plugin Registry

The [plugin registry](https://github.com/hivemind-os/plugin-registry) is a curated index that powers the in-app connector browser. Users can install plugins directly from the "Add Connector" wizard.

### Steps

1. **Fork** `hivemind-os/plugin-registry`
2. **Add** your plugin to `registry.json`:

```json
{
  "name": "@yourscope/hivemind-connector-myservice",
  "displayName": "My Service",
  "description": "Connect to My Service",
  "npmPackage": "@yourscope/hivemind-connector-myservice",
  "categories": ["productivity"],
  "author": "your-github-username",
  "icon": "https://your-cdn/icon.svg"
}
```

3. **Submit a PR** — the CI will automatically:
   - Verify the npm package exists
   - Check the `hivemind` manifest
   - Run basic security checks
4. **Maintainers review** and merge

### Verified Badge

After your plugin is reviewed by maintainers, it gets a "Verified" badge in the connector browser. This means:
- Code has been reviewed for security
- Plugin follows best practices
- It's safe to install

## Versioning

Follow [semver](https://semver.org/):

- **Patch** (1.0.x): Bug fixes, no API changes
- **Minor** (1.x.0): New tools, new config fields (backward compatible)
- **Major** (x.0.0): Breaking changes (removed tools, changed config schema)

Users see update notifications in the connector browser.

## Publishing first-party packages from this repo

The packages in this monorepo keep a local SDK dependency for development and CI. For releases, use the repo's GitHub Actions workflows instead of publishing them by hand from a working copy:

- `sdk-v<version>` → publishes `packages/plugin-sdk`
- `plugin-github-issues-v<version>` → publishes `packages/sample-plugins/github-issues`
- `test-plugin-v<version>` → publishes `packages/test-plugin`

Those workflows validate the tag, build and test the package, and stage a clean npm publish directory with the SDK dependency rewritten to the released semver version.

You can also run the publish workflows manually from the **Actions** tab with `workflow_dispatch` if you prefer not to trigger releases by tag push.

## Best Practices

1. **Write tests** — use the SDK test harness
2. **Document your tools** — good descriptions help the AI agent use them correctly
3. **Handle errors gracefully** — don't crash on transient API failures
4. **Validate on activate** — check credentials in `onActivate`
5. **Include a README** — explain what the plugin does, setup steps, examples
6. **Use .secret() for sensitive fields** — never store tokens in plain config
7. **Declare permissions honestly** — users see them at install time
