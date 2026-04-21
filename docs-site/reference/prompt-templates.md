# Prompt Templates

Prompt templates are reusable, parameterized prompts defined on [personas](/concepts/personas). They use **Handlebars** syntax to insert variables, toggle optional sections, and iterate over lists — turning a single template into many different prompts depending on the parameters you supply.

You can invoke a prompt template in several ways:

- **Chat** — type `/prompt template-id` (or `/p template-id`) in the chat input
- **Agent Stage** — select a template from the persona's prompt list
- **Workflows** — reference a template as a step in a workflow definition

## Template Syntax

HiveMind OS renders templates with the [Handlebars](https://handlebarsjs.com/) templating engine. Only built-in Handlebars helpers are available — no custom helpers are registered.

### Variable Substitution

Use double curly braces to insert a parameter value:

```text
Please explain the following {{language}} code:
```

::: v-pre
At render time, `{{language}}` is replaced with the value the user provides (or the schema default). Variable names must match keys defined in the template's `input_schema`.
:::

### Conditional Blocks

::: v-pre
Wrap optional sections with `{{#if}}...{{/if}}` so they only appear when the parameter has a value:
:::

```text
{{#if spec}}
## Specification
{{spec}}
{{/if}}
```

If `spec` is empty, `null`, or not provided, the entire block is omitted from the rendered output.

### Iteration

::: v-pre
Loop over arrays with `{{#each}}...{{/each}}`:
:::

```text
{{#each items}}
- {{this}}
{{/each}}
```

::: v-pre
Inside the block, `{{this}}` refers to the current item. For arrays of objects you can access properties directly (e.g., `{{this.name}}`).
:::

### Negative Conditionals

::: v-pre
`{{#unless}}` is the inverse of `{{#if}}` — the block renders only when the value is falsy:
:::

```text
{{#unless custom_instructions}}
Use the default analysis guidelines.
{{/unless}}
```

## Input Schema

Every template can declare an `input_schema` that describes its parameters using [JSON Schema](https://json-schema.org/). The schema serves three purposes:

1. **UI form generation** — HiveMind OS builds a form from the schema so users can fill in parameters before the template renders.
2. **Defaults** — properties with a `default` value are pre-filled automatically.
3. **Validation** — `required` fields must be supplied; basic type checks are enforced.

```yaml
input_schema:
  type: object
  properties:
    language:
      type: string
      description: Programming language of the code snippet
      default: rust
    code:
      type: string
      description: The code to explain
    depth:
      type: string
      description: Explanation depth
      default: intermediate
  required:
    - code
```

In this example, `code` is required — the user must provide it. `language` and `depth` have defaults that are used when the user leaves them blank.

::: tip
Always provide a `description` for each property. It is displayed as helper text in the parameter form, making templates much easier for others to use.
:::

## Runtime Behavior

### Strict Mode

The template engine runs in **strict mode**. If a template references a variable that is not present in the input *and* has no schema default, rendering fails with an error rather than silently inserting an empty string. This prevents subtle bugs where a missing parameter goes unnoticed.

::: v-pre
::: warning
Wrap optional parameters in `{{#if}}` blocks. In strict mode, referencing a variable that was not provided — even inside prose — will cause an error unless it is guarded by a conditional.
:::
:::

### Schema Defaults

Before the template is rendered, HiveMind OS merges the schema's `default` values into the supplied parameters. If the user omits `language` in the example above, the renderer automatically fills it with `"rust"`.

### Type Coercion

If the schema declares a property as `type: string` but the supplied value is a number or boolean, the renderer converts it to a string before substitution. This means you can safely accept numeric inputs without worrying about type mismatches in the rendered output.

## Examples

### Explain Code

A simple template that uses variable substitution and schema defaults.

```yaml
prompts:
  - id: explain-code
    name: Explain Code
    description: Get a detailed explanation of a code snippet
    template: |
      Please explain the following {{language}} code in detail:
      ```{{language}}
      {{code}}
      ```
      Explain at a {{depth}} level.
    input_schema:
      type: object
      properties:
        code:
          type: string
          description: The code to explain
        language:
          type: string
          description: Programming language
          default: rust
        depth:
          type: string
          description: Explanation depth (beginner, intermediate, expert)
          default: intermediate
      required:
        - code
```

### Feature Planner

Uses conditional blocks to include optional context sections only when provided.

```yaml
prompts:
  - id: plan-feature
    name: Plan Feature
    description: Create a detailed plan for a software feature
    template: |
      Plan the following software feature.

      ## Feature Description
      {{feature_description}}

      {{#if spec}}
      ## Specification
      {{spec}}
      {{/if}}

      {{#if research_findings}}
      ## Technical Research Findings
      {{research_findings}}
      {{/if}}
    input_schema:
      type: object
      properties:
        feature_description:
          type: string
          description: What the feature should do
        spec:
          type: string
          description: Optional specification or requirements document
        research_findings:
          type: string
          description: Optional prior research or technical notes
      required:
        - feature_description
```

### Capital Gains Calculator

Combines required parameters, a default value, and an optional section.

```yaml
prompts:
  - id: capital-gains
    name: Capital Gains Calculator
    description: Calculate cost basis and capital gains/losses
    template: |
      Calculate the cost basis and capital gains/losses for the following transactions.

      ## Transactions
      {{transactions_description}}

      ## Method
      {{method}}

      {{#if tax_year}}
      ## Tax Year
      {{tax_year}}
      {{/if}}
    input_schema:
      type: object
      properties:
        transactions_description:
          type: string
          description: Description of buy/sell transactions
        method:
          type: string
          description: Accounting method (FIFO, LIFO, specific-id)
          default: FIFO
        tax_year:
          type: string
          description: Limit analysis to a specific tax year
      required:
        - transactions_description
```

## Tips

::: v-pre
::: tip Best Practices
- **Test templates before deploying.** Use `/prompt` in chat to verify the output looks right with different parameter combinations.
- **Use `{{#if}}` for optional parameters.** Strict mode will reject any unguarded reference to a missing variable.
- **Provide defaults in the schema.** This reduces friction — users only need to fill in what they want to change.
- **Write clear descriptions.** The `description` field on both the template and each property helps users understand what to provide.
- **Keep templates focused.** One template per task is easier to maintain than a mega-template that tries to do everything.
:::
:::

## See Also

- [Personas Guide](/guides/personas) — Creating personas and adding templates
- [Slash Commands](/reference/slash-commands) — Using `/prompt` to invoke templates
- [Concepts → Personas](/concepts/personas) — How personas work
