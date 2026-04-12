/**
 * MCP Registry API client, types, and mapping logic.
 *
 * Talks to the public registry at https://registry.modelcontextprotocol.io
 * and converts registry entries into our McpServerConfig format.
 */

import type { McpServerConfig, McpHeaderValue } from '../types';

// ---------------------------------------------------------------------------
// Registry API Types
// ---------------------------------------------------------------------------

export interface RegistryServerJSON {
  name: string;
  title?: string;
  description: string;
  version: string;
  icons?: RegistryIcon[];
  packages?: RegistryPackage[];
  remotes?: RegistryTransport[];
  repository?: { url: string; source: string; subfolder?: string };
  websiteUrl?: string;
}

export interface RegistryIcon {
  src: string;
  mime_type?: string;
  sizes?: string[];
  theme?: 'light' | 'dark';
}

export interface RegistryPackage {
  registryType: string;
  identifier: string;
  version?: string;
  transport: RegistryTransport;
  runtimeHint?: string;
  runtimeArguments?: RegistryArgument[];
  packageArguments?: RegistryArgument[];
  environmentVariables?: RegistryKeyValueInput[];
}

export interface RegistryTransport {
  type: string;
  url?: string;
  headers?: RegistryKeyValueInput[];
  variables?: Record<string, RegistryInput>;
}

export interface RegistryArgument {
  type: string;
  name?: string;
  value?: string;
  valueHint?: string;
  description?: string;
  format?: string;
  isRequired?: boolean;
  isSecret?: boolean;
  isRepeated?: boolean;
  default?: string;
  choices?: string[];
  placeholder?: string;
  variables?: Record<string, RegistryInput>;
}

export interface RegistryInput {
  description?: string;
  format?: string;
  isRequired?: boolean;
  isSecret?: boolean;
  default?: string;
  choices?: string[];
  placeholder?: string;
  value?: string;
}

export interface RegistryKeyValueInput extends RegistryInput {
  name: string;
  variables?: Record<string, RegistryInput>;
}

export interface RegistryServerResponse {
  server: RegistryServerJSON;
  _meta?: {
    'io.modelcontextprotocol.registry/official'?: {
      status: string;
      isLatest: boolean;
      publishedAt: string;
      updatedAt: string;
    };
  };
}

export interface RegistrySearchResponse {
  servers: RegistryServerResponse[];
  metadata: {
    count: number;
    nextCursor?: string;
  };
}

// ---------------------------------------------------------------------------
// API Client
// ---------------------------------------------------------------------------

const REGISTRY_BASE = 'https://registry.modelcontextprotocol.io/v0.1';

export async function searchRegistryServers(
  params: { search?: string; cursor?: string; limit?: number },
  signal?: AbortSignal,
): Promise<RegistrySearchResponse> {
  const url = new URL(`${REGISTRY_BASE}/servers`);
  url.searchParams.set('version', 'latest');
  if (params.search) url.searchParams.set('search', params.search);
  if (params.cursor) url.searchParams.set('cursor', params.cursor);
  url.searchParams.set('limit', String(params.limit ?? 30));

  const resp = await fetch(url.toString(), { signal });
  if (!resp.ok) throw new Error(`Registry search failed: ${resp.status}`);
  return resp.json();
}

// ---------------------------------------------------------------------------
// Required Inputs Collector
// ---------------------------------------------------------------------------

export interface UserPromptInput {
  key: string;
  label: string;
  description?: string;
  isSecret: boolean;
  isRequired: boolean;
  defaultValue?: string;
  choices?: string[];
  placeholder?: string;
  source: 'env' | 'header' | 'argument' | 'variable';
}

const VARIABLE_PATTERN = /\{([^}]+)\}/g;

function hasUnresolvedVariables(value: string): boolean {
  return /\{([^}]+)\}/.test(value);
}

function inputsFromRegistryInput(
  input: RegistryInput,
  key: string,
  label: string,
  source: UserPromptInput['source'],
): UserPromptInput | null {
  if (input.value !== undefined && input.value !== '' && !hasUnresolvedVariables(input.value)) {
    return null;
  }
  return {
    key,
    label,
    description: input.description,
    isSecret: input.isSecret ?? false,
    isRequired: input.isRequired ?? false,
    defaultValue: input.default,
    choices: input.choices,
    placeholder: input.placeholder,
    source,
  };
}

function collectVariableInputs(
  variables: Record<string, RegistryInput> | undefined,
  parentKey: string,
  source: UserPromptInput['source'],
): UserPromptInput[] {
  if (!variables) return [];
  const results: UserPromptInput[] = [];
  for (const [name, input] of Object.entries(variables)) {
    const prompt = inputsFromRegistryInput(input, `${parentKey}.${name}`, name, source);
    if (prompt) results.push(prompt);
  }
  return results;
}

export function collectRequiredInputs(pkg: RegistryPackage): UserPromptInput[] {
  const inputs: UserPromptInput[] = [];
  const seen = new Set<string>();

  const addUnique = (input: UserPromptInput) => {
    if (!seen.has(input.key)) {
      seen.add(input.key);
      inputs.push(input);
    }
  };

  // Environment variables without a set value or with {variable} patterns
  if (pkg.environmentVariables) {
    for (const envVar of pkg.environmentVariables) {
      const prompt = inputsFromRegistryInput(envVar, `env.${envVar.name}`, envVar.name, 'env');
      if (prompt) addUnique(prompt);
      for (const vi of collectVariableInputs(envVar.variables, `env.${envVar.name}`, 'variable')) {
        addUnique(vi);
      }
    }
  }

  // Package arguments with variables that have no value
  if (pkg.packageArguments) {
    for (const arg of pkg.packageArguments) {
      for (const vi of collectVariableInputs(arg.variables, `arg.${arg.name ?? arg.type}`, 'argument')) {
        addUnique(vi);
      }
    }
  }

  // Runtime arguments with variables that have no value
  if (pkg.runtimeArguments) {
    for (const arg of pkg.runtimeArguments) {
      for (const vi of collectVariableInputs(arg.variables, `runtime.${arg.name ?? arg.type}`, 'argument')) {
        addUnique(vi);
      }
    }
  }

  return inputs;
}

export function collectRequiredInputsForRemote(remote: RegistryTransport): UserPromptInput[] {
  const inputs: UserPromptInput[] = [];
  const seen = new Set<string>();

  const addUnique = (input: UserPromptInput) => {
    if (!seen.has(input.key)) {
      seen.add(input.key);
      inputs.push(input);
    }
  };

  // Headers without a value or with {variable} patterns
  if (remote.headers) {
    for (const header of remote.headers) {
      const prompt = inputsFromRegistryInput(header, `header.${header.name}`, header.name, 'header');
      if (prompt) addUnique(prompt);
      for (const vi of collectVariableInputs(header.variables, `header.${header.name}`, 'variable')) {
        addUnique(vi);
      }
    }
  }

  // Transport-level variables
  for (const vi of collectVariableInputs(remote.variables, 'transport', 'variable')) {
    addUnique(vi);
  }

  return inputs;
}

// ---------------------------------------------------------------------------
// Value Resolution Helpers
// ---------------------------------------------------------------------------

export function resolveValue(template: string, userInputs: Record<string, string>): string {
  return template.replace(VARIABLE_PATTERN, (_match, varName: string) => {
    // Look up by exact variable name, or by dotted-prefix keys
    if (varName in userInputs) return userInputs[varName];
    // Try partial match for keys like "env.VAR_NAME.varName"
    for (const [key, val] of Object.entries(userInputs)) {
      if (key.endsWith(`.${varName}`)) return val;
    }
    return '';
  });
}

function resolveArgValue(arg: RegistryArgument, userInputs: Record<string, string>): string {
  const raw = arg.value ?? arg.default ?? '';
  return resolveValue(raw, userInputs);
}

export function buildPackageArgs(
  args: RegistryArgument[] | undefined,
  userInputs: Record<string, string>,
): string[] {
  if (!args) return [];
  const result: string[] = [];
  for (const arg of args) {
    const value = resolveArgValue(arg, userInputs);
    if (!value && arg.isRequired) {
      throw new Error(`Required package argument '${arg.name ?? arg.type}' resolved to an empty value`);
    }
    if (!value) continue;

    if (arg.type === 'positional') {
      result.push(value);
    } else {
      // named argument
      if (arg.name) {
        result.push(arg.name, value);
      }
    }
  }
  return result;
}

export function buildRuntimeArgs(
  args: RegistryArgument[] | undefined,
  userInputs: Record<string, string>,
): string[] {
  if (!args) return [];
  const result: string[] = [];
  for (const arg of args) {
    const value = resolveArgValue(arg, userInputs);
    if (!value && arg.isRequired) {
      throw new Error(`Required runtime argument '${arg.name ?? arg.type}' resolved to an empty value`);
    }
    if (!value) continue;

    if (arg.type === 'positional') {
      result.push(value);
    } else {
      if (arg.name) {
        result.push(arg.name, value);
      }
    }
  }
  return result;
}

// ---------------------------------------------------------------------------
// Mapping Functions
// ---------------------------------------------------------------------------

function buildEnvMap(
  envVars: RegistryKeyValueInput[] | undefined,
  userInputs: Record<string, string>,
): Record<string, string> {
  if (!envVars) return {};
  const env: Record<string, string> = {};
  for (const envVar of envVars) {
    const raw = envVar.value ?? userInputs[`env.${envVar.name}`] ?? envVar.default ?? '';
    env[envVar.name] = resolveValue(raw, userInputs);
  }
  return env;
}

function slugify(name: string): string {
  // Extract the part after the last '/'
  const parts = name.split('/');
  const base = parts[parts.length - 1] || parts[0];
  return base
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-|-$/g, '');
}

export function mapRegistryPackageToConfig(
  server: RegistryServerJSON,
  pkg: RegistryPackage,
  userInputs: Record<string, string>,
): Partial<McpServerConfig> {
  const id = slugify(server.name);
  const runtimeArgs = buildRuntimeArgs(pkg.runtimeArguments, userInputs);
  const pkgArgs = buildPackageArgs(pkg.packageArguments, userInputs);

  switch (pkg.registryType) {
    case 'npm': {
      const versionSuffix = pkg.version ? `@${pkg.version}` : '@latest';
      return {
        id,
        transport: 'stdio',
        command: pkg.runtimeHint || 'npx',
        args: [...runtimeArgs, '-y', `${pkg.identifier}${versionSuffix}`, ...pkgArgs],
        url: null,
        env: buildEnvMap(pkg.environmentVariables, userInputs),
        headers: {},
      };
    }
    case 'pypi': {
      const versionSuffix = pkg.version ? `==${pkg.version}` : '';
      return {
        id,
        transport: 'stdio',
        command: pkg.runtimeHint || 'uvx',
        args: [...runtimeArgs, `${pkg.identifier}${versionSuffix}`, ...pkgArgs],
        url: null,
        env: buildEnvMap(pkg.environmentVariables, userInputs),
        headers: {},
      };
    }
    case 'oci': {
      // For OCI, env vars go as -e args to docker, not in the env map
      const envFlags: string[] = [];
      if (pkg.environmentVariables) {
        for (const envVar of pkg.environmentVariables) {
          const raw = envVar.value ?? userInputs[`env.${envVar.name}`] ?? envVar.default ?? '';
          const resolved = resolveValue(raw, userInputs);
          if (resolved) {
            envFlags.push('-e', `${envVar.name}=${resolved}`);
          }
        }
      }
      const imageRef = pkg.version ? `${pkg.identifier}:${pkg.version}` : pkg.identifier;
      return {
        id,
        transport: 'stdio',
        command: pkg.runtimeHint || 'docker',
        args: [...runtimeArgs, 'run', '-i', '--rm', ...envFlags, imageRef, ...pkgArgs],
        url: null,
        env: {},
        headers: {},
      };
    }
    case 'nuget': {
      return {
        id,
        transport: 'stdio',
        command: pkg.runtimeHint || 'dotnet',
        args: [...runtimeArgs, 'tool', 'run', pkg.identifier, '--', ...pkgArgs],
        url: null,
        env: buildEnvMap(pkg.environmentVariables, userInputs),
        headers: {},
      };
    }
    default: {
      // Fallback for unknown registry types
      return {
        id,
        transport: 'stdio',
        command: pkg.runtimeHint || pkg.identifier,
        args: [...runtimeArgs, ...pkgArgs],
        url: null,
        env: buildEnvMap(pkg.environmentVariables, userInputs),
        headers: {},
      };
    }
  }
}

export function mapRegistryRemoteToConfig(
  server: RegistryServerJSON,
  remote: RegistryTransport,
  userInputs: Record<string, string>,
): Partial<McpServerConfig> {
  const id = slugify(server.name);
  const transport = remote.type as McpServerConfig['transport'];

  // Build headers, resolving variables and mapping isSecret → secret-ref
  const headers: Record<string, McpHeaderValue> = {};
  if (remote.headers) {
    for (const header of remote.headers) {
      const raw = header.value ?? userInputs[`header.${header.name}`] ?? header.default ?? '';
      const resolved = resolveValue(raw, userInputs);
      headers[header.name] = {
        type: header.isSecret ? 'secret-ref' : 'plain',
        value: resolved,
      };
    }
  }

  return {
    id,
    transport,
    command: null,
    args: [],
    url: remote.url ? resolveValue(remote.url, userInputs) : null,
    env: {},
    headers,
  };
}

// ---------------------------------------------------------------------------
// Variant Description Helper
// ---------------------------------------------------------------------------

export type RegistryVariant =
  | { kind: 'package'; pkg: RegistryPackage; label: string }
  | { kind: 'remote'; remote: RegistryTransport; label: string };

export function getVariants(server: RegistryServerJSON): RegistryVariant[] {
  const variants: RegistryVariant[] = [];

  if (server.packages) {
    for (const pkg of server.packages) {
      const transport = pkg.transport?.type ?? 'stdio';
      const label = `${pkg.registryType} · ${pkg.identifier} (${transport})`;
      variants.push({ kind: 'package', pkg, label });
    }
  }

  if (server.remotes) {
    for (const remote of server.remotes) {
      const label = `${remote.type} · ${remote.url ?? 'unknown'}`;
      variants.push({ kind: 'remote', remote, label });
    }
  }

  return variants;
}

// ---------------------------------------------------------------------------
// ID Generation
// ---------------------------------------------------------------------------

export function generateServerId(serverName: string, existingIds: string[]): string {
  const base = slugify(serverName);
  if (!base) return generateUniqueId('mcp-server', existingIds);
  return generateUniqueId(base, existingIds);
}

function generateUniqueId(base: string, existingIds: string[]): string {
  if (!existingIds.includes(base)) return base;
  let counter = 2;
  while (existingIds.includes(`${base}-${counter}`)) {
    counter++;
  }
  return `${base}-${counter}`;
}
