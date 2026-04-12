# hive-classification

Data classification engine that labels, gates, and redacts data to enforce privacy boundaries in HiveMind OS. It provides a four-tier classification model, a pluggable labelling pipeline, a gating function that controls cross-boundary data flow, and a sanitizer that redacts sensitive content.

## Classification Model

Every piece of data is assigned one of four classification levels, ordered from least to most restrictive:

```
Public → Internal → Confidential → Restricted
```

| Level | Description |
|---|---|
| **Public** | Safe for external or unrestricted use. |
| **Internal** | For internal use; not sensitive but not intended for public release. |
| **Confidential** | Contains sensitive information requiring access controls. |
| **Restricted** | Highest sensitivity — strictest handling and redaction rules apply. |

The classification level determines what can happen to the data: where it may flow, whether it must be redacted, and which channels are permitted to receive it.

## Modules

### `model`

Core data types used across the crate.

- **`DataClass`** — enum representing the four classification tiers.
- **`ChannelClass`** — classification level assigned to an output channel.
- **`SensitiveSpan`** — byte range within content that has been identified as sensitive.
- **`LabelSource`** — indicates how a classification was determined (pattern match, source origin, etc.).

### `labeller`

Classification pipeline that inspects data and assigns labels.

- **`Labeller`** — trait implemented by all labelling strategies.
- **`LabellerPipeline`** — chains multiple labellers and returns a combined result.
- **`PatternLabeller`** — regex-based detector that scans content for sensitive patterns (e.g. API keys, emails).
- **`SourceLabeller`** — assigns classification based on the data's origin rather than its content.
- **`ClassificationResult`** — aggregate output of the pipeline including the resolved `DataClass` and any detections.
- **`Detection`** — a single finding produced by a labeller, including the matched span and source.

### `gate`

Decision engine that determines whether data is allowed to flow from one classification level to a channel.

- **`gate()`** — core function: given a `DataClass` and a `ChannelClass`, returns a `GateDecision`.
- **`GateDecision`** — outcome of a gate check (allow, deny, or allow-with-redaction).
- **`OverridePolicy`** — policy that can relax or tighten the default gating behaviour.
- **`OverrideAction`** — specific action taken when an override policy applies.

The basic rule is simple: data cannot flow to a channel whose classification level is lower than the data's own level. For example, `Confidential` data is blocked from a `Public` channel unless an override policy permits it (typically requiring redaction first).

### `sanitizer`

Content redaction utilities that strip or mask sensitive spans before data leaves a trusted boundary.

- **`redact()`** — removes or replaces all `SensitiveSpan`s in a piece of content.
- **`RedactionResult`** — the sanitized content along with metadata about what was redacted.

## Dependencies

| Crate | Purpose |
|---|---|
| `regex` | Pattern matching in `PatternLabeller` |
| `serde` | Serialization / deserialization of classification types |

This is a **leaf crate** — it has no dependencies on other workspace crates.
