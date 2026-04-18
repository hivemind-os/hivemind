import type { PromptSchemaField } from '~/components/PromptSchemaEditor';

export const parseSchemaFields = (schema: Record<string, any> | undefined): PromptSchemaField[] => {
  if (!schema?.properties) return [];
  const required: string[] = schema.required ?? [];

  function parseProps(props: Record<string, any>, req: string[]): PromptSchemaField[] {
    return Object.entries(props).map(([name, prop]) => {
      const p = prop as any;
      const fieldType = (p.type ?? 'string') as PromptSchemaField['varType'];
      let defaultValue = '';
      if (p.default !== undefined) {
        defaultValue = fieldType === 'string' ? String(p.default) : JSON.stringify(p.default);
      }
      const field: PromptSchemaField = {
        name,
        varType: fieldType,
        description: p.description ?? '',
        required: req.includes(name),
        defaultValue,
        enumValues: Array.isArray(p.enum) ? p.enum : [],
      };
      if (p.minLength != null) field.minLength = p.minLength;
      if (p.maxLength != null) field.maxLength = p.maxLength;
      if (p.pattern != null) field.pattern = p.pattern;
      if (p.minimum != null) field.minimum = p.minimum;
      if (p.maximum != null) field.maximum = p.maximum;
      if (p['x-ui']) field.xUi = { ...p['x-ui'] };
      if (fieldType === 'object' && p.properties) {
        field.properties = parseProps(p.properties, p.required ?? []);
      }
      if (fieldType === 'array' && p.items) {
        field.itemsType = (p.items.type ?? 'string') as string;
        if (p.items.type === 'object' && p.items.properties) {
          field.itemProperties = parseProps(p.items.properties, p.items.required ?? []);
        }
      }
      return field;
    });
  }

  return parseProps(schema.properties as Record<string, any>, required);
};

export const buildSchemaFromFields = (fields: PromptSchemaField[]): Record<string, any> | undefined => {
  if (fields.length === 0) return undefined;

  function buildProps(flds: PromptSchemaField[]): { properties: Record<string, any>; required: string[] } {
    const properties: Record<string, any> = {};
    const required: string[] = [];
    for (const f of flds) {
      const prop: Record<string, any> = { type: f.varType };
      if (f.description) prop.description = f.description;
      if (f.defaultValue) {
        if (f.varType === 'string') {
          prop.default = f.defaultValue;
        } else {
          try { prop.default = JSON.parse(f.defaultValue); } catch { prop.default = f.defaultValue; }
        }
      }
      if (f.enumValues && f.enumValues.length > 0) {
        prop.enum = f.enumValues;
      }
      if (f.xUi && Object.values(f.xUi).some(val => val !== undefined)) {
        prop['x-ui'] = { ...f.xUi };
      }
      if (f.minLength != null) prop.minLength = f.minLength;
      if (f.maxLength != null) prop.maxLength = f.maxLength;
      if (f.pattern) prop.pattern = f.pattern;
      if (f.minimum != null) prop.minimum = f.minimum;
      if (f.maximum != null) prop.maximum = f.maximum;
      if (f.varType === 'object' && f.properties && f.properties.length > 0) {
        const nested = buildProps(f.properties);
        prop.properties = nested.properties;
        if (nested.required.length > 0) prop.required = nested.required;
      }
      if (f.varType === 'array') {
        const items: Record<string, any> = { type: f.itemsType ?? 'string' };
        if (f.itemsType === 'object' && f.itemProperties && f.itemProperties.length > 0) {
          const nested = buildProps(f.itemProperties);
          items.properties = nested.properties;
          if (nested.required.length > 0) items.required = nested.required;
        }
        prop.items = items;
      }
      properties[f.name] = prop;
      if (f.required) required.push(f.name);
    }
    return { properties, required };
  }

  const { properties, required } = buildProps(fields);
  const schema: Record<string, any> = { type: 'object', properties };
  if (required.length > 0) schema.required = required;
  return schema;
};

export function computePreview(templateText: string, fields: PromptSchemaField[]): { text?: string; error?: string } {
  if (!templateText.trim()) return { text: '(empty template)' };
  try {
    const defaults: Record<string, any> = {};
    for (const f of fields) {
      if (f.defaultValue) {
        if (f.varType === 'string') {
          defaults[f.name] = f.defaultValue;
        } else {
          try { defaults[f.name] = JSON.parse(f.defaultValue); } catch { defaults[f.name] = f.defaultValue; }
        }
      } else {
        defaults[f.name] = `<${f.name}>`;
      }
    }
    const text = templateText.replace(/\{\{\s*([^#/!>][^}]*?)\s*\}\}/g, (_, key) => {
      const trimmed = key.trim();
      return trimmed in defaults ? String(defaults[trimmed]) : `{{${trimmed}}}`;
    });
    return { text };
  } catch (e: any) {
    return { error: e.message };
  }
}
