/**
 * Zod extensions for Hivemind plugin config schemas.
 *
 * Re-exports standard Zod and adds hivemind-specific schema metadata
 * via a `.hivemind()` extension that stores UI hints (labels, sections,
 * secret flags, help text) in Zod's `.describe()` metadata.
 *
 * The host reads this metadata to render config forms in the desktop UI.
 */
import { z as zod } from "zod";
// ─── Metadata Storage ───────────────────────────────────────────────────────
const HIVEMIND_META = Symbol("hivemind_meta");
export function getFieldMeta(schema) {
    return schema[HIVEMIND_META] ?? {};
}
function withMeta(schema, meta) {
    const existing = getFieldMeta(schema);
    const clone = schema.describe(meta.label ?? existing.label ?? schema.description ?? "");
    clone[HIVEMIND_META] = { ...existing, ...meta };
    return clone;
}
// Patch ZodType prototype with our extension methods
const proto = zod.ZodType.prototype;
proto.label = function (text) {
    return withMeta(this, { label: text });
};
proto.helpText = function (text) {
    return withMeta(this, { helpText: text });
};
proto.section = function (name) {
    return withMeta(this, { section: name });
};
proto.secret = function () {
    return withMeta(this, { secret: true });
};
proto.radio = function () {
    return withMeta(this, { radio: true });
};
proto.placeholder = function () {
    return withMeta(this, { placeholder: arguments[0] });
};
/**
 * Serialize a Zod config schema to a JSON-serializable format
 * that the Rust host can parse to render config forms.
 */
export function serializeConfigSchema(schema) {
    const shape = schema._def?.shape?.();
    if (!shape) {
        return { type: "object", properties: {}, required: [] };
    }
    const properties = {};
    const required = [];
    for (const [key, fieldSchema] of Object.entries(shape)) {
        const field = fieldSchema;
        properties[key] = serializeField(field);
        // Check if required (not optional, not having a default)
        if (!isOptional(field)) {
            required.push(key);
        }
    }
    return { type: "object", properties, required };
}
function serializeField(schema) {
    const meta = getFieldMeta(schema);
    const def = schema._def;
    const result = {
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
            if (check.kind === "min")
                result.minimum = check.value;
            if (check.kind === "max")
                result.maximum = check.value;
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
function inferType(schema) {
    const def = schema._def;
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
function isOptional(schema) {
    const def = schema._def;
    const typeName = def?.typeName;
    return (typeName === "ZodOptional" ||
        typeName === "ZodDefault" ||
        (typeName === "ZodNullable" && isOptional(def.innerType)));
}
function extractDefault(schema) {
    const def = schema._def;
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
function extractEnum(schema) {
    const def = schema._def;
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
//# sourceMappingURL=schema.js.map