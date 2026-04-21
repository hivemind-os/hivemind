# @hivemind-os/test-plugin

Host-API exercise plugin for Hivemind.

This package is used by the plugin test harness and end-to-end coverage to verify that the host and SDK still interoperate correctly.

## Development

```bash
npm install
npm run build
npm test
```

## Local linking

```bash
hivemind plugin link .
```

It is publishable to npm, but it is primarily intended for validation and advanced examples rather than normal end-user installation.
