/**
 * Plugin runtime — bootstraps the plugin process and wires up the
 * JSON-RPC transport, PluginContext, tool dispatch, and lifecycle hooks.
 *
 * This is the "main loop" of a plugin process. When a plugin calls
 * `definePlugin(...)`, the runtime takes over and handles all host
 * communication automatically.
 */
import { JsonRpcTransport } from "./transport.js";
import { serializeConfigSchema } from "./schema.js";
// ─── Runtime Bootstrap ──────────────────────────────────────────────────────
export function startPluginRuntime(definition) {
    const transport = new JsonRpcTransport(process.stdin, process.stdout);
    let context = null;
    let loopAbortController = null;
    let loopPromise = null;
    const scheduledHandlers = new Map();
    // ── Host → Plugin: initialize ───────────────────────────────
    transport.onMethod("initialize", async (params) => {
        const { pluginId, config, hostInfo } = params;
        context = createContext(transport, pluginId, config, hostInfo);
        return { capabilities: buildCapabilities(definition) };
    });
    // ── Host → Plugin: config schema ───────────────────────────
    transport.onMethod("plugin/configSchema", async () => {
        return serializeConfigSchema(definition.configSchema);
    });
    // ── Host → Plugin: validate config ──────────────────────────
    transport.onMethod("plugin/validateConfig", async (params) => {
        const result = definition.configSchema.safeParse(params?.config);
        if (result.success) {
            return { valid: true };
        }
        return {
            valid: false,
            errors: result.error.issues.map((issue) => ({
                path: issue.path.join("."),
                message: issue.message,
            })),
        };
    });
    // ── Host → Plugin: activate ─────────────────────────────────
    transport.onMethod("plugin/activate", async (params) => {
        if (!context)
            throw new Error("Plugin not initialized");
        // Update config if provided
        if (params?.config) {
            context._config = params.config;
        }
        if (definition.onActivate) {
            await definition.onActivate(context);
        }
        return { ok: true };
    });
    // ── Host → Plugin: deactivate ───────────────────────────────
    transport.onMethod("plugin/deactivate", async () => {
        if (!context)
            throw new Error("Plugin not initialized");
        // Stop loop if running
        if (loopAbortController) {
            loopAbortController.abort();
            await loopPromise?.catch(() => { });
            loopAbortController = null;
            loopPromise = null;
        }
        if (definition.onDeactivate) {
            await definition.onDeactivate(context);
        }
        return { ok: true };
    });
    // ── Host → Plugin: start loop ───────────────────────────────
    transport.onMethod("plugin/startLoop", async () => {
        if (!context)
            throw new Error("Plugin not initialized");
        if (!definition.loop)
            throw new Error("Plugin has no loop defined");
        if (loopAbortController)
            throw new Error("Loop already running");
        loopAbortController = new AbortController();
        const loopContext = {
            ...context,
            signal: loopAbortController.signal,
        };
        loopPromise = definition.loop(loopContext).catch((err) => {
            if (err?.name !== "AbortError") {
                context?.logger.error("Loop crashed", { error: String(err) });
                transport.notify("plugin/loopCrashed", {
                    error: err instanceof Error ? err.message : String(err),
                });
            }
        });
        return { ok: true };
    });
    // ── Host → Plugin: stop loop ────────────────────────────────
    transport.onMethod("plugin/stopLoop", async () => {
        if (!loopAbortController)
            return { ok: true, wasRunning: false };
        loopAbortController.abort();
        await loopPromise?.catch(() => { });
        loopAbortController = null;
        loopPromise = null;
        return { ok: true, wasRunning: true };
    });
    // ── Host → Plugin: status ───────────────────────────────────
    transport.onMethod("plugin/status", async () => {
        return {
            initialized: context !== null,
            loopRunning: loopAbortController !== null,
        };
    });
    // ── Host → Plugin: tools/list (MCP-compatible) ──────────────
    transport.onMethod("tools/list", async () => {
        return {
            tools: definition.tools.map(serializeTool),
        };
    });
    // ── Host → Plugin: tools/call (MCP-compatible) ──────────────
    transport.onMethod("tools/call", async (params) => {
        if (!context)
            throw new Error("Plugin not initialized");
        const { name, arguments: args } = params;
        const tool = definition.tools.find((t) => t.name === name);
        if (!tool)
            throw new Error(`Unknown tool: ${name}`);
        // Validate params against the tool's Zod schema
        const parsed = tool.parameters.parse(args ?? {});
        const result = await tool.execute(parsed, context);
        return result;
    });
    // ── Host → Plugin: scheduled task callback ──────────────────
    transport.onMethod("plugin/scheduledTask", async (params) => {
        const handler = scheduledHandlers.get(params?.id);
        if (!handler)
            throw new Error(`Unknown scheduled task: ${params?.id}`);
        await handler();
        return { ok: true };
    });
    // Start the transport
    transport.start();
    // Signal readiness
    transport.notify("plugin/ready", {});
}
// ─── Context Factory ────────────────────────────────────────────────────────
function createContext(transport, pluginId, config, hostInfo) {
    const neverAbort = new AbortController();
    const ctx = {
        pluginId,
        get config() {
            return ctx._config ?? config;
        },
        signal: neverAbort.signal,
        // ── Messaging ───────────────────────────────────────────
        async emitMessage(msg) {
            await transport.request("host/emitMessage", { message: msg });
        },
        async emitMessages(msgs) {
            await transport.request("host/emitMessages", { messages: msgs });
        },
        // ── Secrets ─────────────────────────────────────────────
        secrets: {
            async get(key) {
                const result = (await transport.request("host/secretGet", {
                    key,
                }));
                return result?.value ?? null;
            },
            async set(key, value) {
                await transport.request("host/secretSet", { key, value });
            },
            async delete(key) {
                await transport.request("host/secretDelete", { key });
            },
            async has(key) {
                const result = (await transport.request("host/secretHas", {
                    key,
                }));
                return result?.exists ?? false;
            },
        },
        // ── Persistent Store ────────────────────────────────────
        store: {
            async get(key) {
                const result = (await transport.request("host/storeGet", {
                    key,
                }));
                return result?.value ?? null;
            },
            async set(key, value) {
                await transport.request("host/storeSet", { key, value });
            },
            async delete(key) {
                await transport.request("host/storeDelete", { key });
            },
            async keys() {
                const result = (await transport.request("host/storeKeys", {}));
                return result?.keys ?? [];
            },
        },
        // ── Logger ──────────────────────────────────────────────
        logger: {
            debug(msg, data) {
                transport.notify("host/log", { level: "debug", msg, data });
            },
            info(msg, data) {
                transport.notify("host/log", { level: "info", msg, data });
            },
            warn(msg, data) {
                transport.notify("host/log", { level: "warn", msg, data });
            },
            error(msg, data) {
                transport.notify("host/log", { level: "error", msg, data });
            },
        },
        // ── Notifications ───────────────────────────────────────
        async notify(notification) {
            await transport.request("host/notify", notification);
        },
        // ── Events ──────────────────────────────────────────────
        async emitEvent(eventType, payload) {
            await transport.request("host/emitEvent", { eventType, payload });
        },
        // ── Status ──────────────────────────────────────────────
        async updateStatus(status) {
            await transport.request("host/updateStatus", status);
        },
        // ── Timers ──────────────────────────────────────────────
        sleep(ms) {
            return new Promise((resolve, reject) => {
                const timer = setTimeout(resolve, ms);
                // If we have a loop signal, listen for abort
                if (ctx.signal.aborted) {
                    clearTimeout(timer);
                    reject(new DOMException("Aborted", "AbortError"));
                    return;
                }
                const onAbort = () => {
                    clearTimeout(timer);
                    reject(new DOMException("Aborted", "AbortError"));
                };
                ctx.signal.addEventListener("abort", onAbort, { once: true });
            });
        },
        async schedule(opts) {
            // Store handler locally, register interval with host
            ctx._scheduledHandlers?.set(opts.id, opts.handler);
            await transport.request("host/schedule", {
                id: opts.id,
                intervalSeconds: opts.intervalSeconds,
            });
        },
        async unschedule(id) {
            ctx._scheduledHandlers?.delete(id);
            await transport.request("host/unschedule", { id });
        },
        // ── HTTP ────────────────────────────────────────────────
        http: {
            async fetch(url, init) {
                const result = (await transport.request("host/httpFetch", {
                    url,
                    method: init?.method ?? "GET",
                    headers: init?.headers
                        ? Object.fromEntries(init.headers instanceof Headers
                            ? init.headers.entries()
                            : Object.entries(init.headers))
                        : undefined,
                    body: typeof init?.body === "string" ? init.body : undefined,
                }));
                return new Response(result?.body ?? null, {
                    status: result?.status ?? 200,
                    statusText: result?.statusText ?? "OK",
                    headers: result?.headers ?? {},
                });
            },
        },
        // ── Data Directory ──────────────────────────────────────
        dataDir: {
            async resolve(path) {
                const result = (await transport.request("host/fsResolve", {
                    path,
                }));
                return result?.absolutePath ?? "";
            },
            async readFile(path) {
                const result = (await transport.request("host/fsRead", {
                    path,
                }));
                return result?.content ?? "";
            },
            async writeFile(path, content) {
                const encoded = typeof content === "string"
                    ? content
                    : Buffer.from(content).toString("base64");
                await transport.request("host/fsWrite", {
                    path,
                    content: encoded,
                    encoding: typeof content === "string" ? "utf8" : "base64",
                });
            },
            async readDir(path) {
                const result = (await transport.request("host/fsReadDir", {
                    path,
                }));
                return result?.entries ?? [];
            },
            async exists(path) {
                const result = (await transport.request("host/fsExists", {
                    path,
                }));
                return result?.exists ?? false;
            },
            async mkdir(path) {
                await transport.request("host/fsMkdir", { path });
            },
            async remove(path) {
                await transport.request("host/fsRemove", { path });
            },
        },
        // ── Host Info ───────────────────────────────────────────
        host: {
            version: hostInfo?.version ?? "0.0.0",
            platform: hostInfo?.platform ?? "linux",
            capabilities: hostInfo?.capabilities ?? [],
        },
        // ── Discovery ───────────────────────────────────────────
        connectors: {
            async list() {
                const result = (await transport.request("host/listConnectors", {}));
                return result?.connectors ?? [];
            },
        },
        personas: {
            async list() {
                const result = (await transport.request("host/listPersonas", {}));
                return result?.personas ?? [];
            },
        },
    };
    // Store internal state
    ctx._config = config;
    ctx._scheduledHandlers = new Map();
    return ctx;
}
// ─── Helpers ────────────────────────────────────────────────────────────────
function buildCapabilities(definition) {
    const caps = ["tools"];
    if (definition.loop)
        caps.push("loop");
    if (definition.auth)
        caps.push("auth");
    if (definition.onActivate)
        caps.push("lifecycle");
    return caps;
}
function serializeTool(tool) {
    const schema = tool.parameters;
    // Convert Zod schema to JSON Schema
    const jsonSchema = zodToJsonSchema(schema);
    return {
        name: tool.name,
        description: tool.description,
        inputSchema: jsonSchema,
        annotations: tool.annotations ?? {},
    };
}
function zodToJsonSchema(schema) {
    const def = schema._def;
    const typeName = def?.typeName;
    switch (typeName) {
        case "ZodObject": {
            const shape = def.shape();
            const properties = {};
            const required = [];
            for (const [key, fieldSchema] of Object.entries(shape)) {
                properties[key] = zodToJsonSchema(fieldSchema);
                if (!fieldSchema._def?.typeName?.includes("Optional") &&
                    !fieldSchema._def?.typeName?.includes("Default")) {
                    required.push(key);
                }
            }
            return { type: "object", properties, required };
        }
        case "ZodString":
            return {
                type: "string",
                ...(schema.description ? { description: schema.description } : {}),
            };
        case "ZodNumber": {
            const result = {
                type: "number",
                ...(schema.description ? { description: schema.description } : {}),
            };
            for (const check of def.checks ?? []) {
                if (check.kind === "min")
                    result.minimum = check.value;
                if (check.kind === "max")
                    result.maximum = check.value;
            }
            return result;
        }
        case "ZodBoolean":
            return {
                type: "boolean",
                ...(schema.description ? { description: schema.description } : {}),
            };
        case "ZodEnum":
            return {
                type: "string",
                enum: def.values,
                ...(schema.description ? { description: schema.description } : {}),
            };
        case "ZodArray":
            return {
                type: "array",
                items: zodToJsonSchema(def.type),
            };
        case "ZodOptional":
            return zodToJsonSchema(def.innerType);
        case "ZodDefault": {
            const inner = zodToJsonSchema(def.innerType);
            const defaultVal = typeof def.defaultValue === "function"
                ? def.defaultValue()
                : def.defaultValue;
            return { ...inner, default: defaultVal };
        }
        case "ZodRecord":
            return {
                type: "object",
                additionalProperties: def.valueType
                    ? zodToJsonSchema(def.valueType)
                    : true,
            };
        case "ZodAny":
            return {};
        default:
            return { type: "string" };
    }
}
//# sourceMappingURL=runtime.js.map