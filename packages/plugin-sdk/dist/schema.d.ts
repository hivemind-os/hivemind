/**
 * Zod extensions for Hivemind plugin config schemas.
 *
 * Re-exports standard Zod and adds hivemind-specific schema metadata
 * via a `.hivemind()` extension that stores UI hints (labels, sections,
 * secret flags, help text) in Zod's `.describe()` metadata.
 *
 * The host reads this metadata to render config forms in the desktop UI.
 */
import { z as zod, type ZodTypeAny } from "zod";
export interface HivemindFieldMeta {
    label?: string;
    helpText?: string;
    section?: string;
    secret?: boolean;
    radio?: boolean;
    placeholder?: string;
}
export declare function getFieldMeta(schema: ZodTypeAny): HivemindFieldMeta;
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
export declare function serializeConfigSchema(schema: ZodTypeAny): SerializedConfigSchema;
export { zod as z };
//# sourceMappingURL=schema.d.ts.map