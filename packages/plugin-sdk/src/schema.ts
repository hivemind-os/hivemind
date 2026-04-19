/**
 * Zod extensions for Hivemind plugin config schemas.
 *
 * Re-exports standard Zod and adds hivemind-specific schema metadata
 * via a `.hivemind()` extension that stores UI hints (labels, sections,
 * secret flags, help text) in Zod's `.describe()` metadata.
 *
 * The host reads this metadata to render config forms in the desktop UI.
 */

import { z as zod, type ZodTypeAny, type ZodType } from "zod";

// ─── Metadata Storage ───────────────────────────────────────────────────────

const HIVEMIND_META = Symbol("hivemind_meta");

export interface HivemindFieldMeta {
  label?: string;
  helpText?: string;
  section?: string;
  secret?: boolean;
  radio?: boolean;
  placeholder?: string;
}

export function getFieldMeta(schema: ZodTypeAny): HivemindFieldMeta {
  return (schema as any)[HIVEMIND_META] ?? {};
}

function withMeta<T extends ZodTypeAny>(
  schema: T,
  meta: Partial<HivemindFieldMeta>,
): T {
  const existing = getFieldMeta(schema);
  const clone = schema.describe(
    meta.label ?? existing.label ?? schema.description ?? "",
  );
  (clone as any)[HIVEMIND_META] = { ...existing, ...meta };
  return clone as T;
}

// ─── Schema Extension Methods ───────────────────────────────────────────────

declare module "zod" {
  interface ZodType {
    /** Set the UI label for this field. */
    label(text: string): this;
    /** Set help text shown as a tooltip. */
    helpText(text: string): this;
    /** Group this field into a named section in the config form. */
    section(name: string): this;
    /** Mark this field as a secret (rendered as password input, stored in keyring). */
    secret(): this;
    /** Render enum as radio buttons instead of a dropdown. */
    radio(): this;
    /** Set placeholder text for the input. */
    placeholder(text: string): this;
  }
}

// Patch ZodType prototype with our extension methods
const proto = zod.ZodType.prototype;

proto.label = function (this: ZodTypeAny, text: string) {
  return withMeta(this, { label: text });
};

proto.helpText = function (this: ZodTypeAny, text: string) {
  return withMeta(this, { helpText: text });
};

proto.section = function (this: ZodTypeAny, name: string) {
  return withMeta(this, { section: name });
};

proto.secret = function (this: ZodTypeAny) {
  return withMeta(this, { secret: true });
};

proto.radio = function (this: ZodTypeAny) {
  return withMeta(this, { radio: true });
};

proto.placeholder = function (this: ZodTypeAny) {
  return withMeta(this, { placeholder: arguments[0] });
};

// ─── Schema Serialization ───────────────────────────────────────────────────

export interface SerializedConfigSchema {
  type: "object";
  properties: Record<string, SerializedFieldSchema>;
  required: string[];
}

export interface SerializedFieldSchema {
  type: string;
  description?: string;
  default?: unknown;
  enum?: unknown[];
  minimum?: number;
  maximum?: number;
  items?: SerializedFieldSchema;
  hivemind?: HivemindFieldMeta;
}

/**
 * Serialize a Zod config schema to a JSON-serializable format
 * that the Rust host can parse to render config forms.
 */
export function serializeConfigSchema(
  schema: ZodTypeAny,
): SerializedConfigSchema {
  const shape = (schema as any)._def?.shape?.();
  if (!shape) {
    return { type: "object", properties: {}, required: [] };
  }

  const properties: Record<string, SerializedFieldSchema> = {};
  const required: string[] = [];

  for (const [key, fieldSchema] of Object.entries(shape)) {
    const field = fieldSchema as ZodTypeAny;
    properties[key] = serializeField(field);

    // Check if required (not optional, not having a default)
    if (!isOptional(field)) {
      required.push(key);
    }
  }

  return { type: "object", properties, required };
}

function serializeField(schema: ZodTypeAny): SerializedFieldSchema {
  const meta = getFieldMeta(schema);
  const def = (schema as any)._def;
  const result: SerializedFieldSchema = {
    type: inferType(schema),
  };

  if (schema.description) {
    result.description = schema.description;
  }

  // Extract default value
  const defaultVal = extractDefault(schema);
  if (defaultVal !== undefined) {
    result.default = defaultVal;
  }

  // Extract enum values
  const enumVals = extractEnum(schema);
  if (enumVals) {
    result.enum = enumVals;
  }

  // Extract min/max for numbers
  if (def?.checks) {
    for (const check of def.checks) {
      if (check.kind === "min") result.minimum = check.value;
      if (check.kind === "max") result.maximum = check.value;
    }
  }

  // Extract array items
  if (def?.typeName === "ZodArray" && def.type) {
    result.items = serializeField(def.type);
  }

  // Attach hivemind metadata if present
  if (Object.keys(meta).length > 0) {
    result.hivemind = meta;
  }

  return result;
}

function inferType(schema: ZodTypeAny): string {
  const def = (schema as any)._def;
  const typeName = def?.typeName;

  switch (typeName) {
    case "ZodString":
      return "string";
    case "ZodNumber":
      return "number";
    case "ZodBoolean":
      return "boolean";
    case "ZodArray":
      return "array";
    case "ZodEnum":
      return "string";
    case "ZodOptional":
      return inferType(def.innerType);
    case "ZodDefault":
      return inferType(def.innerType);
    case "ZodNullable":
      return inferType(def.innerType);
    default:
      return "string";
  }
}

function isOptional(schema: ZodTypeAny): boolean {
  const def = (schema as any)._def;
  const typeName = def?.typeName;
  return (
    typeName === "ZodOptional" ||
    typeName === "ZodDefault" ||
    (typeName === "ZodNullable" && isOptional(def.innerType))
  );
}

function extractDefault(schema: ZodTypeAny): unknown | undefined {
  const def = (schema as any)._def;
  if (def?.typeName === "ZodDefault") {
    return typeof def.defaultValue === "function"
      ? def.defaultValue()
      : def.defaultValue;
  }
  if (def?.innerType) {
    return extractDefault(def.innerType);
  }
  return undefined;
}

function extractEnum(schema: ZodTypeAny): unknown[] | undefined {
  const def = (schema as any)._def;
  if (def?.typeName === "ZodEnum") {
    return def.values;
  }
  if (def?.innerType) {
    return extractEnum(def.innerType);
  }
  return undefined;
}

// Re-export zod with our extensions applied
export { zod as z };
