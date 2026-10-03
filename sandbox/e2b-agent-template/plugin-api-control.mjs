#!/usr/bin/env node
import { createHash, randomBytes, randomUUID, timingSafeEqual } from "node:crypto";
import { readFileSync } from "node:fs";
import {
  chmod,
  chown,
  lstat,
  mkdir,
  open,
  readFile,
  readdir,
  realpath,
  rename,
  rm,
} from "node:fs/promises";
import { spawn, spawnSync } from "node:child_process";
import { dirname, join } from "node:path";

const ROOT = process.env.SUPERAGENT_PLUGIN_CONTROL_ROOT || "/var/lib/superagent/plugin-control";
const CAPABILITY_FILE = process.env.SUPERAGENT_PLUGIN_CONTROL_CAPABILITY_FILE || "/home/user/.superagent/plugin-control.capability";
const BINDING_ROOT = process.env.SUPERAGENT_PLUGIN_BINDING_ROOT || "/run/vectra-plugin-bindings";
const SERVER_ARTIFACT = "/opt/vectra/plugin-api/plugin-api-server-0.1.0.mjs";
const BRIDGE_ARTIFACT = "/opt/vectra/plugin-api/plugin-bridge-0.1.0.mjs";
const CATALOG = "/opt/vectra/plugin-api/catalog.json";
const NODE = "/opt/vectra/plugin-api/node/bin/node";
const SERVICE_UID_BASE = Number(process.env.SUPERAGENT_PLUGIN_SERVICE_UID_BASE || 20000);
const PORT_BASE = Number(process.env.SUPERAGENT_PLUGIN_PORT_BASE || 4400);
const SLOT_COUNT = 128;
const MAX_INPUT = 512 * 1024;
const idPattern = /^[a-z][a-z0-9-]{0,63}$/;
const targetIdPattern = /^[A-Za-z0-9_-]{1,128}$/;

function digest(value) {
  return createHash("sha256").update(value).digest("hex");
}

function assertArtifact(path) {
  const bytes = readFileSync(path);
  const expected = readFileSync(`${path}.sha256`, "utf8").trim().split(/\s+/)[0];
  if (!/^[a-f0-9]{64}$/.test(expected) || digest(bytes) !== expected) {
    throw new Error("Plugin API artifact checksum mismatch");
  }
}

function safeId(value, label, pattern = idPattern) {
  if (typeof value !== "string" || !pattern.test(value)) {
    throw new Error(`Invalid ${label}`);
  }
  return value;
}

async function writePrivate(path, bytes, uid = 0, gid = 0) {
  await mkdir(dirname(path), { recursive: true, mode: 0o700 });
  const temporary = `${path}.${randomUUID()}.tmp`;
  const file = await open(temporary, "wx", 0o600);
  try {
    await file.writeFile(bytes);
    await file.sync();
  } finally {
    await file.close();
  }
  await chmod(temporary, 0o600);
  await chown(temporary, uid, gid);
  try {
    await rename(temporary, path);
  } catch (error) {
    await rm(temporary, { force: true });
    throw error;
  }
}

async function readJson(path) {
  try {
    return JSON.parse(await readFile(path, "utf8"));
  } catch (error) {
    if (error?.code === "ENOENT") return null;
    throw error;
  }
}

async function readInput() {
  const chunks = [];
  let size = 0;
  for await (const chunk of process.stdin) {
    size += chunk.length;
    if (size > MAX_INPUT) throw new Error("Plugin control request exceeds 512 KiB");
    chunks.push(chunk);
  }
  const value = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("Invalid plugin control request");
  }
  return value;
}

function localSandboxId() {
  const envId = (process.env.E2B_SANDBOX_ID || process.env.SUPERAGENT_SANDBOX_ID || "").trim();
  if (envId) return envId;
  try {
    return readFileSync("/run/e2b/.E2B_SANDBOX_ID", "utf8").trim();
  } catch {
    return "";
  }
}

function sandboxAccount() {
  const ids = ["-u", "-g"].map((flag) => {
    const account = spawnSync("/usr/bin/id", [flag, "user"], { encoding: "utf8" });
    const id = Number(account.stdout.trim());
    if (account.status !== 0 || !Number.isSafeInteger(id) || id <= 0) {
      throw new Error("Sandbox daemon account is unavailable");
    }
    return id;
  });
  return { uid: ids[0], gid: ids[1] };
}

function assertAuthority(request) {
  const { uid } = sandboxAccount();
  if (
    process.env.SUDO_USER &&
    (process.env.SUDO_USER !== "user" || process.env.SUDO_UID !== String(uid))
  ) {
    throw new Error("Plugin control must be invoked by the sandbox daemon account");
  }
  let expectedCapability = "";
  try {
    expectedCapability = readFileSync(CAPABILITY_FILE, "utf8").trim();
  } catch {}
  const suppliedCapability = typeof request.capability === "string" ? request.capability : "";
  const expectedBytes = Buffer.from(expectedCapability);
  const suppliedBytes = Buffer.from(suppliedCapability);
  if (
    expectedBytes.length < 32 ||
    expectedBytes.length > 256 ||
    suppliedBytes.length !== expectedBytes.length ||
    !timingSafeEqual(suppliedBytes, expectedBytes)
  ) {
    throw new Error("Plugin control capability is invalid");
  }
  const expectedInstance = readFileSync("/home/user/.superagent/plugin-instance.id", "utf8").trim();
  if (!/^[A-Za-z0-9_-]{1,128}$/.test(expectedInstance) || request.instanceId !== expectedInstance) {
    throw new Error("Plugin control instance identity mismatch");
  }
  const sandboxId = localSandboxId();
  if (!sandboxId || request.sandboxId !== sandboxId) {
    throw new Error("Plugin control sandbox generation mismatch");
  }
  safeId(request.bot?.id, "bot ID", targetIdPattern);
  if (!Number.isSafeInteger(request.bot?.revision) || request.bot.revision < 1) {
    throw new Error("Invalid bot revision");
  }
}

async function canonicalWorkspace(raw) {
  if (typeof raw !== "string" || !raw.startsWith("/home/user/workspace")) {
    throw new Error("Bot workspace is outside the sandbox workspace");
  }
  const root = await realpath("/home/user/workspace");
  const target = await realpath(raw);
  if (target !== root && !target.startsWith(`${root}/`)) {
    throw new Error("Bot workspace resolves outside the sandbox workspace");
  }
  return target;
}

const statePath = (botId) => join(ROOT, "bots", botId, "control.json");
const serviceRoot = (botId) => join(ROOT, "services", botId);
const serviceConfigPath = (botId) => join(serviceRoot(botId), "config.json");
const servicePidPath = (botId) => join(serviceRoot(botId), "service.pid.json");
const conversationRoot = (botId, sessionId) => join(BINDING_ROOT, botId, sessionId);
const generationRoot = (botId, sessionId, generation) =>
  join(conversationRoot(botId, sessionId), generation);

async function existingAllocations() {
  const used = new Set();
  try {
    for (const entry of await readdir(join(ROOT, "bots"), { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const state = await readJson(statePath(entry.name));
      if (Number.isInteger(state?.slot)) used.add(state.slot);
    }
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  return used;
}

async function newState(request, workspace) {
  const used = await existingAllocations();
  let slot = -1;
  for (let candidate = 0; candidate < SLOT_COUNT; candidate++) {
    if (!used.has(candidate)) {
      slot = candidate;
      break;
    }
  }
  if (slot < 0) throw new Error("Plugin API bot capacity reached");
  return {
    schemaVersion: 1,
    instanceId: request.instanceId,
    sandboxId: request.sandboxId,
    botId: request.bot.id,
    botRevision: request.bot.revision,
    workspace,
    slot,
    serviceUid: SERVICE_UID_BASE + slot,
    agentUid: sandboxAccount().uid,
    port: PORT_BASE + slot,
    managementToken: randomBytes(32).toString("hex"),
    bindings: {},
    configDigest: "",
    updatedAt: new Date().toISOString(),
  };
}

async function loadState(request, workspace) {
  let state = await readJson(statePath(request.bot.id));
  if (!state) state = await newState(request, workspace);
  if (
    state.schemaVersion !== 1 ||
    state.instanceId !== request.instanceId ||
    state.sandboxId !== request.sandboxId ||
    state.botId !== request.bot.id ||
    !Number.isSafeInteger(state.serviceUid) ||
    state.agentUid !== sandboxAccount().uid ||
    !Number.isSafeInteger(state.port) ||
    !state.managementToken ||
    !state.bindings ||
    Array.isArray(state.bindings)
  ) {
    throw new Error("Stored Plugin API target identity is invalid");
  }
  if (request.bot.revision < state.botRevision) throw new Error("Bot revision is stale");
  state.botRevision = request.bot.revision;
  state.workspace = workspace;
  return state;
}

async function saveState(state) {
  state.updatedAt = new Date().toISOString();
  await writePrivate(statePath(state.botId), Buffer.from(JSON.stringify(state, null, 2)));
}

function allowedCommands() {
  const defaults = {
    node: NODE,
    python3: "/usr/bin/python3",
    uv: "/usr/local/bin/uv",
    bun: "/usr/local/bin/bun",
  };
  const configured = process.env.SUPERAGENT_PLUGIN_API_ALLOWED_COMMANDS?.trim();
  if (!configured) {
    return Object.fromEntries(
      Object.entries(defaults).filter(([, path]) => spawnSync("test", ["-x", path]).status === 0),
    );
  }
  const parsed = JSON.parse(configured);
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error("Invalid Plugin API command allowlist");
  }
  return parsed;
}

function allowedOrigins() {
  return [
    "https://api.githubcopilot.com",
    ...(process.env.SUPERAGENT_PLUGIN_API_ALLOWED_ORIGINS || "")
      .split(",")
      .map((value) => value.trim())
      .filter(Boolean),
  ];
}

function activeBindings(state) {
  return Object.values(state.bindings).filter(
    (binding) =>
      binding.status !== "removing" &&
      binding.status !== "removed" &&
      Date.parse(binding.policyExpiresAt) > Date.now(),
  );
}

function serviceConfig(state) {
  return {
    dataDir: join(serviceRoot(state.botId), "data"),
    host: "127.0.0.1",
    port: state.port,
    credentials: [
      {
        tokenSha256: digest(state.managementToken),
        scopes: ["read", "manage"],
        workspaceIds: ["workspace"],
      },
      ...activeBindings(state).map((binding) => ({
        tokenSha256: digest(binding.bridgeToken),
        scopes: ["read", "execute"],
        workspaceIds: ["workspace"],
        allowedPluginIds: [binding.pluginId],
        allowedMcpServers: { [binding.pluginId]: [binding.serverId] },
        executionBoundary: "isolated-account",
        agentUid: state.agentUid,
      })),
    ],
    workspaces: { workspace: state.workspace },
    localSourceRoots: [],
    allowedCommands: allowedCommands(),
    allowedMcpOrigins: allowedOrigins(),
  };
}

async function chownTree(path, uid, gid) {
  const info = await lstat(path);
  if (info.isSymbolicLink()) throw new Error("Plugin API path contains a symbolic link");
  await chown(path, uid, gid);
  if (!info.isDirectory()) return;
  for (const entry of await readdir(path, { withFileTypes: true })) {
    if (entry.isSymbolicLink()) throw new Error("Plugin API path contains a symbolic link");
    await chownTree(join(path, entry.name), uid, gid);
  }
}

function processStart(pid) {
  try {
    const raw = readFileSync(`/proc/${pid}/stat`, "utf8");
    return raw.slice(raw.lastIndexOf(") ") + 2).split(" ")[19] || "";
  } catch {
    return "";
  }
}

function processMatches(record, configPath) {
  if (!record?.pid || !record?.startTime || processStart(record.pid) !== record.startTime) return false;
  try {
    const cmdline = readFileSync(`/proc/${record.pid}/cmdline`, "utf8").replaceAll("\0", " ");
    return cmdline.includes(SERVER_ARTIFACT) && cmdline.includes(configPath);
  } catch {
    return false;
  }
}

async function stopService(state) {
  const configPath = serviceConfigPath(state.botId);
  const record = await readJson(servicePidPath(state.botId));
  if (!processMatches(record, configPath)) return;
  try {
    process.kill(record.pid, "SIGTERM");
  } catch {
    return;
  }
  for (let i = 0; i < 20 && processStart(record.pid) === record.startTime; i++) {
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  if (processStart(record.pid) === record.startTime) {
    try {
      process.kill(record.pid, "SIGKILL");
    } catch {}
  }
}

async function grantWorkspace(state) {
  if (spawnSync("setfacl", ["-m", `u:${state.serviceUid}:--x`, "/home/user"], { stdio: "ignore" }).status !== 0) {
    throw new Error("Could not grant isolated bot traversal to its workspace");
  }
  for (const uid of [state.agentUid, state.serviceUid]) {
    for (const args of [
      ["-m", `u:${uid}:rwx`, state.workspace],
      ["-m", `d:u:${uid}:rwx`, state.workspace],
    ]) {
      if (spawnSync("setfacl", args, { stdio: "ignore" }).status !== 0) {
        throw new Error("Could not grant isolated bot access to its workspace");
      }
    }
  }
}

async function ensureService(state, forceRestart = false) {
  await mkdir(ROOT, { recursive: true, mode: 0o711 });
  await chmod(ROOT, 0o711);
  await mkdir(join(ROOT, "bots"), { recursive: true, mode: 0o700 });
  await chmod(join(ROOT, "bots"), 0o700);
  await mkdir(join(ROOT, "services"), { recursive: true, mode: 0o711 });
  await chmod(join(ROOT, "services"), 0o711);
  for (const path of [serviceRoot(state.botId), join(serviceRoot(state.botId), "data")]) {
    await mkdir(path, { recursive: true, mode: 0o700 });
    await chmod(path, 0o700);
  }
  await grantWorkspace(state);
  await chownTree(serviceRoot(state.botId), state.serviceUid, state.serviceUid);
  const config = Buffer.from(JSON.stringify(serviceConfig(state), null, 2));
  const configDigest = digest(config);
  const configPath = serviceConfigPath(state.botId);
  const pidRecord = await readJson(servicePidPath(state.botId));
  const alive = processMatches(pidRecord, configPath);
  if (alive && !forceRestart && state.configDigest === configDigest) {
    try {
      await waitHealthy(state);
      return;
    } catch {}
  }
  if (alive) await stopService(state);
  await writePrivate(configPath, config, state.serviceUid, state.serviceUid);
  await chownTree(serviceRoot(state.botId), state.serviceUid, state.serviceUid);
  const logPath = join(serviceRoot(state.botId), "service.log");
  const log = await open(logPath, "a", 0o600);
  const child = spawn(
    "setpriv",
    [
      `--reuid=${state.serviceUid}`,
      `--regid=${state.serviceUid}`,
      "--clear-groups",
      NODE,
      SERVER_ARTIFACT,
      "--config",
      configPath,
    ],
    {
      detached: true,
      cwd: serviceRoot(state.botId),
      env: {
        PATH: "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        HOME: serviceRoot(state.botId),
        LANG: "C.UTF-8",
      },
      stdio: ["ignore", log.fd, log.fd],
    },
  );
  child.unref();
  await log.close();
  const pid = child.pid;
  if (!pid) throw new Error("Plugin API service did not return a PID");
  let startTime = "";
  for (let i = 0; i < 20 && !startTime; i++) {
    startTime = processStart(pid);
    if (!startTime) await new Promise((resolve) => setTimeout(resolve, 25));
  }
  if (!startTime) throw new Error("Plugin API service exited during startup");
  await writePrivate(
    servicePidPath(state.botId),
    Buffer.from(JSON.stringify({ pid, startTime })),
  );
  state.configDigest = configDigest;
  await saveState(state);
  await waitHealthy(state);
}

async function waitHealthy(state) {
  const url = `http://127.0.0.1:${state.port}/v1/version`;
  let last = "service did not start";
  for (let i = 0; i < 60; i++) {
    try {
      const response = await fetch(url, {
        headers: { authorization: `Bearer ${state.managementToken}` },
        signal: AbortSignal.timeout(500),
      });
      if (response.ok && (await response.json()).apiVersion === "1") return;
      last = `HTTP ${response.status}`;
    } catch (error) {
      last = error.message;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`Plugin API service unavailable: ${last}`);
}

async function api(state, path, init = {}) {
  const headers = new Headers(init.headers || {});
  headers.set("authorization", `Bearer ${state.managementToken}`);
  headers.set("accept", "application/json");
  if (init.body !== undefined) headers.set("content-type", "application/json");
  const response = await fetch(`http://127.0.0.1:${state.port}${path}`, {
    ...init,
    headers,
    redirect: "error",
    signal: AbortSignal.timeout(35_000),
    body: init.body === undefined ? undefined : JSON.stringify(init.body),
  });
  let body = {};
  try {
    body = await response.json();
  } catch {}
  if (!response.ok) {
    throw new Error(
      `Plugin API ${body?.error?.code || "request_failed"} (HTTP ${response.status})`,
    );
  }
  return body;
}

async function operation(state, initial) {
  let current = initial;
  const deadline = Date.now() + 125_000;
  while (current.status === "running") {
    if (Date.now() >= deadline) {
      throw new Error("Plugin API operation is still running; inspect status before retrying");
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
    current = await api(state, `/v1/operations/${encodeURIComponent(current.id)}`);
  }
  if (current.status !== "succeeded") {
    throw new Error(`Plugin API ${current.error?.code || current.status}`);
  }
  return current.result;
}

function publicBinding(binding) {
  if (!binding) return { configured: false, revision: null, status: "removed", tools: [] };
  const { descriptor: _descriptor, bridgeToken: _bridgeToken, ...safe } = binding;
  return { configured: binding.status !== "removed", ...safe };
}

function publicState(state) {
  return {
    service: {
      running: true,
      allowedCommands: Object.keys(allowedCommands()),
      allowedMcpOrigins: allowedOrigins(),
    },
    inventory: null,
    bindings: Object.fromEntries(
      Object.entries(state.bindings).map(([sessionId, binding]) => [sessionId, publicBinding(binding)]),
    ),
    agentUid: state.agentUid,
  };
}

async function ensureBindingDirectory(state, sessionId, generation) {
  const { gid } = sandboxAccount();
  await mkdir(BINDING_ROOT, { recursive: true, mode: 0o711 });
  await chmod(BINDING_ROOT, 0o711);
  const botRoot = join(BINDING_ROOT, state.botId);
  const sessionRoot = conversationRoot(state.botId, sessionId);
  const root = generationRoot(state.botId, sessionId, generation);
  for (const path of [botRoot, sessionRoot, root]) {
    await mkdir(path, { recursive: true, mode: 0o700 });
    await chmod(path, 0o700);
    await chown(path, state.agentUid, gid);
  }
  return root;
}

async function materializeBinding(state, binding) {
  const root = await ensureBindingDirectory(state, binding.sessionId, binding.generation);
  const tokenFile = join(root, "credential");
  const bindingFile = join(root, "binding.json");
  const value = {
    schemaVersion: 1,
    revision: binding.revision,
    target: {
      kind: "sandbox",
      owner: state.instanceId,
      id: state.botId,
      generation: state.sandboxId,
      launchSlot: binding.sessionId,
      agentSessionId: binding.agentSessionId,
    },
    baseUrl: `http://127.0.0.1:${state.port}/`,
    tokenFile,
    workspaceId: "workspace",
    pluginDigests: { [binding.pluginId]: binding.releaseDigest },
    policy: {
      expiresAt: binding.policyExpiresAt,
      servers: [
        {
          pluginId: binding.pluginId,
          serverId: binding.serverId,
          releaseDigest: binding.releaseDigest,
        },
      ],
    },
  };
  const { gid } = sandboxAccount();
  await writePrivate(tokenFile, Buffer.from(binding.bridgeToken), state.agentUid, gid);
  await writePrivate(bindingFile, Buffer.from(JSON.stringify(value, null, 2)), state.agentUid, gid);
  binding.descriptor = {
    name: binding.serverName,
    command: NODE,
    args: [BRIDGE_ARTIFACT],
    env: [{ name: "SUPERCHARGE_PLUGIN_BRIDGE_BINDING", value: bindingFile }],
  };
}

async function stageBinding(state, request, plugin) {
  const previous = state.bindings[request.sessionId] || null;
  if (!previous && activeBindings(state).length >= 15) {
    throw new Error("This bot has reached the 15 active managed MCP conversation limit");
  }
  if ((request.expectedRevision ?? null) !== (previous?.revision ?? null)) {
    throw new Error("Plugin binding revision changed; refresh before retrying");
  }
  const generation = randomUUID();
  const binding = {
    revision: (previous?.revision || 0) + 1,
    sessionId: request.sessionId,
    agentSessionId: request.agentSessionId,
    pluginId: request.pluginId,
    serverId: request.serverId,
    releaseDigest: plugin.activeDigest,
    generation,
    serverName: `vectra-plugin-api-${digest(`${state.botId}:${request.sessionId}:${generation}`).slice(0, 16)}`,
    policyExpiresAt: new Date(Date.now() + 24 * 60 * 60_000).toISOString(),
    status: "waiting",
    lastError: null,
    tools: [],
    bridgeToken: randomBytes(32).toString("hex"),
    descriptor: null,
  };
  await materializeBinding(state, binding);
  state.bindings[request.sessionId] = binding;
  await saveState(state);
  return publicBinding(binding);
}

function catalogSource(request) {
  safeId(request.catalogId, "catalog plugin ID");
  if (typeof request.catalogSha256 !== "string" || !/^[a-f0-9]{64}$/.test(request.catalogSha256)) {
    throw new Error("Invalid catalog release digest");
  }
  const catalog = JSON.parse(readFileSync(CATALOG, "utf8"));
  if (catalog.schemaVersion !== 1 || !Array.isArray(catalog.plugins)) {
    throw new Error("Managed MCP catalog is unavailable");
  }
  const entry = catalog.plugins.find((plugin) => plugin.id === request.catalogId && plugin.source?.sha256 === request.catalogSha256);
  if (!entry || (Array.isArray(entry.targets) && !entry.targets.includes("supercharge-e2b")) || entry.source.type !== "archive" || !/^https:\/\/raw\.githubusercontent\.com\/iotserver24\/supercharge-app\/[a-f0-9]{40}\/public\/managed-mcp\/[a-z][a-z0-9-]*-\d+\.\d+\.\d+\.tar\.gz$/.test(entry.source.url)) {
    throw new Error("Catalog release is unavailable for this sandbox");
  }
  return { type: "archive", url: entry.source.url, sha256: entry.source.sha256 };
}

async function handle(request) {
  if (process.geteuid?.() !== 0) throw new Error("Plugin controller must run as root");
  assertAuthority(request);
  assertArtifact(SERVER_ARTIFACT);
  assertArtifact(BRIDGE_ARTIFACT);
  const workspace = await canonicalWorkspace(request.bot.defaultFolder);
  const state = await loadState(request, workspace);
  const action = request.action;
  if (action === "status") {
    await ensureService(state);
    const result = publicState(state);
    result.inventory = await api(state, "/v1/plugins");
    return result;
  }
  if (action === "validate") {
    await ensureService(state);
    return api(state, "/v1/plugins/validate", {
      method: "POST",
      body: { manifest: request.manifest },
    });
  }
  if (action === "install" || action === "installCatalog") {
    const source = action === "installCatalog"
      ? catalogSource(request)
      : { type: "inline", manifest: request.manifest };
    await ensureService(state);
    safeId(request.requestId, "request ID", /^[A-Za-z0-9_-]{8,128}$/);
    const initial = await api(state, "/v1/plugins/install", {
      method: "POST",
      headers: { "idempotency-key": request.requestId },
      body: { source },
    });
    return { plugin: await operation(state, initial) };
  }
  if (action === "pluginAction") {
    await ensureService(state);
    safeId(request.pluginId, "plugin ID");
    safeId(request.requestId, "request ID", /^[A-Za-z0-9_-]{8,128}$/);
    const allowed = new Set(["configure", "trust", "enable", "disable", "uninstall", "rollback", "update"]);
    if (!request.pluginAction || !allowed.has(request.pluginAction.action)) {
      throw new Error("Unsupported Plugin API action");
    }
    if (request.pluginAction.action === "update") {
      const source = catalogSource(request);
      if (request.pluginId !== request.catalogId) throw new Error("Catalog update target mismatch");
      request.pluginAction = {
        action: "update",
        expectedRevision: request.pluginAction.expectedRevision,
        source,
      };
    }
    if (
      request.pluginAction.action === "uninstall" &&
      activeBindings(state).some((binding) => binding.pluginId === request.pluginId)
    ) {
      throw new Error("Remove this plugin from its agent sessions before uninstalling it");
    }
    const initial = await api(state, `/v1/plugins/${encodeURIComponent(request.pluginId)}/actions`, {
      method: "POST",
      headers: { "idempotency-key": request.requestId },
      body: request.pluginAction,
    });
    return { plugin: await operation(state, initial) };
  }
  if (action === "stageBinding") {
    await ensureService(state);
    safeId(request.pluginId, "plugin ID");
    safeId(request.serverId, "server ID");
    safeId(request.sessionId, "conversation ID", targetIdPattern);
    safeId(request.agentSessionId, "agent session ID", targetIdPattern);
    const plugin = await api(state, `/v1/plugins/${encodeURIComponent(request.pluginId)}`);
    if (
      !plugin.enabled ||
      !plugin.trusted ||
      !plugin.manifest?.mcpServers?.[request.serverId]
    ) {
      throw new Error("Plugin must be trusted, configured, enabled, and contain the selected MCP server");
    }
    return { binding: await stageBinding(state, request, plugin), agentUid: state.agentUid };
  }
  if (action === "activateBinding" || action === "descriptor") {
    safeId(request.sessionId, "conversation ID", targetIdPattern);
    const binding = state.bindings[request.sessionId] || null;
    if (!binding || binding.status === "removing" || binding.status === "removed") {
      return { binding: publicBinding(binding), descriptor: null, agentUid: state.agentUid };
    }
    if (request.expectedRevision !== undefined && request.expectedRevision !== binding.revision) {
      throw new Error("Plugin binding revision changed; refresh before retrying");
    }
    await ensureService(state);
    await materializeBinding(state, binding);
    await saveState(state);
    return { binding: publicBinding(binding), descriptor: binding.descriptor, agentUid: state.agentUid };
  }
  if (action === "mark") {
    safeId(request.sessionId, "conversation ID", targetIdPattern);
    const binding = state.bindings[request.sessionId];
    if (!binding || binding.revision !== request.expectedRevision) {
      throw new Error("Plugin binding revision changed; refresh before retrying");
    }
    if (!["waiting", "applied", "ready", "failed", "removing", "removed"].includes(request.status)) {
      throw new Error("Invalid plugin binding status");
    }
    binding.status = request.status;
    binding.agentSessionId = request.agentSessionId || binding.agentSessionId;
    binding.tools = Array.isArray(request.tools)
      ? request.tools
          .slice(0, 256)
          .map((tool) => ({
            name: String(tool.name || "").slice(0, 128),
            description:
              typeof tool.description === "string"
                ? tool.description.slice(0, 4096)
                : null,
          }))
          .filter((tool) => tool.name)
      : [];
    binding.lastError =
      typeof request.error === "string" ? request.error.slice(0, 240) : null;
    await saveState(state);
    return { binding: publicBinding(binding) };
  }
  if (action === "beginRemove") {
    safeId(request.sessionId, "conversation ID", targetIdPattern);
    const binding = state.bindings[request.sessionId];
    if (!binding || binding.revision !== request.expectedRevision) {
      throw new Error("Plugin binding revision changed; refresh before retrying");
    }
    binding.revision += 1;
    binding.status = "removing";
    binding.tools = [];
    binding.lastError = null;
    await saveState(state);
    return { binding: publicBinding(binding) };
  }
  if (action === "revokeBinding") {
    safeId(request.sessionId, "conversation ID", targetIdPattern);
    const binding = state.bindings[request.sessionId];
    if (!binding || binding.status !== "removing") {
      throw new Error("Plugin binding is not awaiting removal");
    }
    await ensureService(state, true);
    return { binding: publicBinding(binding) };
  }
  if (action === "finishRemove") {
    safeId(request.sessionId, "conversation ID", targetIdPattern);
    const binding = state.bindings[request.sessionId];
    if (binding) {
      await rm(conversationRoot(state.botId, request.sessionId), {
        recursive: true,
        force: true,
      });
      delete state.bindings[request.sessionId];
      await saveState(state);
    }
    return { binding: publicBinding(null) };
  }
  throw new Error("Unsupported plugin control action");
}

try {
  const request = await readInput();
  const result = await handle(request);
  process.stdout.write(`${JSON.stringify({ ok: true, result })}\n`);
} catch (error) {
  process.stdout.write(
    `${JSON.stringify({ ok: false, error: String(error?.message || error).slice(0, 300) })}\n`,
  );
  process.exitCode = 1;
}
