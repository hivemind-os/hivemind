import { For, Show, createSignal, createMemo, createEffect, onCleanup } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Hourglass, RefreshCw, TriangleAlert, ScrollText, Shield, Search, CheckCircle } from 'lucide-solid';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from '~/ui/dialog';
import { EmptyState } from '~/ui/empty-state';
import { Tabs, TabsList, TabsTrigger } from '~/ui/tabs';
import { Button } from '~/ui/button';
import { ConfirmDialog } from '~/ui/confirm-dialog';
import { Badge } from '~/ui/badge';
import type {
  DiscoveredSkill,
  InstalledSkill,
  SkillAuditResult,
  SkillSourceConfig,
} from '../types';

type SkillsView = 'installed' | 'discover' | 'sources';

interface SkillsTabProps {
  availableModels: { id: string; label: string }[];
  persona_id: string;
}

const SkillsTab = (props: SkillsTabProps) => {
  const [view, setView] = createSignal<SkillsView>('installed');
  const [installedSkills, setInstalledSkills] = createSignal<InstalledSkill[]>([]);
  const [discoveredSkills, setDiscoveredSkills] = createSignal<DiscoveredSkill[]>([]);
  const [sources, setSources] = createSignal<SkillSourceConfig[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [discovering, setDiscovering] = createSignal(false);

  // Audit dialog state
  const [auditTarget, setAuditTarget] = createSignal<DiscoveredSkill | null>(null);
  const [auditModel, setAuditModel] = createSignal('');
  const [auditRunning, setAuditRunning] = createSignal(false);
  const [auditResult, setAuditResult] = createSignal<SkillAuditResult | null>(null);
  const [auditStatus, setAuditStatus] = createSignal('');
  const [confirmUninstall, setConfirmUninstall] = createSignal<string | null>(null);

  // Track audit listener at component scope so onCleanup works
  let auditUnlisten: (() => void) | undefined;
  onCleanup(() => { auditUnlisten?.(); });
  // Discover search
  const [discoverSearch, setDiscoverSearch] = createSignal('');
  const filteredDiscoveredSkills = createMemo(() => {
    const q = discoverSearch().toLowerCase().trim();
    if (!q) return discoveredSkills();
    return discoveredSkills().filter(
      (s) =>
        s.manifest.name.toLowerCase().includes(q) ||
        s.manifest.description.toLowerCase().includes(q) ||
        s.source_id.toLowerCase().includes(q)
    );
  });

  const loadInstalled = async () => {
    try {
      const skills = await invoke<InstalledSkill[]>('skills_list_installed_for_persona', { persona_id: props.persona_id });
      setInstalledSkills(skills);
    } catch (e) {
      console.error('Failed to load installed skills:', e);
    }
  };

  const loadSources = async () => {
    try {
      const srcs = await invoke<SkillSourceConfig[]>('skills_get_sources');
      setSources(srcs);
    } catch (e) {
      console.error('Failed to load skill sources:', e);
    }
  };

  createEffect(() => {
    const _pid = props.persona_id; // track persona changes
    void loadInstalled();
    void loadSources();
  });

  const discoverSkills = async () => {
    setDiscovering(true);
    setError(null);
    try {
      const skills = await invoke<DiscoveredSkill[]>('skills_discover');
      setDiscoveredSkills(skills);
      setView('discover');
    } catch (e: any) {
      setError(e?.toString() ?? 'Discovery failed');
    } finally {
      setDiscovering(false);
    }
  };

  const toggleSkillEnabled = async (name: string, enabled: boolean) => {
    try {
      await invoke('skills_set_enabled_for_persona', { persona_id: props.persona_id, name, enabled });
      await loadInstalled();
    } catch (e) {
      console.error('Failed to toggle skill:', e);
    }
  };

  const uninstallSkill = async (name: string) => {
    try {
      await invoke('skills_uninstall_for_persona', { persona_id: props.persona_id, name });
      await loadInstalled();
    } catch (e) {
      console.error('Failed to uninstall skill:', e);
    }
  };

  const rebuildIndex = async () => {
    setLoading(true);
    setError(null);
    try {
      await invoke('skills_rebuild_index');
      await loadInstalled();
    } catch (e: any) {
      setError(e?.toString() ?? 'Rebuild failed');
    } finally {
      setLoading(false);
    }
  };

  // --- Audit dialog ---

  const startAudit = (skill: DiscoveredSkill) => {
    setAuditResult(null);
    setAuditStatus('');
    setAuditModel('');
    // Defer opening so the triggering click event finishes before the
    // dialog mounts — otherwise Kobalte's DismissableLayer picks up the
    // same pointer-down as an "outside" interaction and closes instantly.
    requestAnimationFrame(() => setAuditTarget(skill));
  };

  const runAudit = async () => {
    const target = auditTarget();
    if (!target || !auditModel()) return;
    setAuditRunning(true);
    setAuditStatus('Starting security audit…');
    setAuditResult(null);

    // Listen for SSE progress events from the Tauri bridge
    auditUnlisten?.(); // clean up any previous listener

    auditUnlisten = await listen<string>('skill:audit', (ev) => {
      try {
        const event = JSON.parse(ev.payload);
        if (event.phase === 'fetching') {
          setAuditStatus('Fetching skill content…');
        } else if (event.phase === 'auditing') {
          setAuditStatus(event.message || 'Running security audit…');
        }
      } catch { /* ignore parse errors */ }
    });

    try {
      const result = await invoke<SkillAuditResult>('skills_audit', {
        source_id: target.source_id,
        source_path: target.source_path,
        model: auditModel(),
      });
      setAuditResult(result);
      setAuditStatus('Audit complete.');
    } catch (e: any) {
      const raw = e?.toString() ?? 'Unknown error';
      let msg = raw;
      if (raw.includes('Could not reach')) {
        msg = 'Could not reach the daemon. Make sure it is running and try again.';
      } else if (raw.includes('timed out') || raw.includes('deadline has elapsed')) {
        msg = 'The audit timed out. The selected model may be slow — try a different one.';
      } else if (raw.includes('model_not_supported') || raw.includes('not supported')) {
        msg = 'The selected model is not available. Please choose a different model.';
      }
      setAuditStatus(`Audit failed: ${msg}`);
    } finally {
      auditUnlisten?.();
      auditUnlisten = undefined;
      setAuditRunning(false);
    }
  };

  const confirmInstall = async () => {
    const target = auditTarget();
    const audit = auditResult();
    const model = auditModel();
    if (!target || !audit) return;
    setAuditRunning(true);
    setAuditStatus('Installing skill...');
    try {
      await invoke('skills_install_for_persona', {
        persona_id: props.persona_id,
        name: target.manifest.name,
        source_id: target.source_id,
        source_path: target.source_path,
        model,
        audit,
      });
      setAuditTarget(null);
      setAuditResult(null);
      await loadInstalled();
      // Refresh discovered to update install status
      if (discoveredSkills().length > 0) {
        void discoverSkills();
      }
    } catch (e: any) {
      const raw = e?.toString() ?? 'Unknown error';
      let msg = raw;
      if (raw.includes('error sending request')) {
        msg = 'Could not reach the daemon. Make sure it is running and try again.';
      } else if (raw.includes('FORBIDDEN') || raw.includes('critical')) {
        msg = 'Installation blocked: the audit found critical security risks.';
      }
      setAuditStatus(`Install failed: ${msg}`);
    } finally {
      setAuditRunning(false);
    }
  };

  // --- Source management ---
  const [newSourceOwner, setNewSourceOwner] = createSignal('');
  const [newSourceRepo, setNewSourceRepo] = createSignal('');

  const addSource = async () => {
    const owner = newSourceOwner().trim();
    const repo = newSourceRepo().trim();
    if (!owner || !repo) return;
    const newSrc: SkillSourceConfig = { type: 'github', owner, repo, enabled: true };
    const updated = [...sources(), newSrc];
    try {
      await invoke('skills_set_sources', { sources: updated });
      setSources(updated);
      setNewSourceOwner('');
      setNewSourceRepo('');
    } catch (e) {
      console.error('Failed to add source:', e);
    }
  };

  const removeSource = async (idx: number) => {
    const updated = sources().filter((_, i) => i !== idx);
    try {
      await invoke('skills_set_sources', { sources: updated });
      setSources(updated);
    } catch (e) {
      console.error('Failed to remove source:', e);
    }
  };

  const severityColor = (severity: string) => {
    switch (severity) {
      case 'critical': return 'hsl(var(--destructive))';
      case 'high': return 'hsl(24 93% 75%)';
      case 'medium': return 'hsl(40 90% 84%)';
      case 'low': return 'hsl(160 60% 76%)';
      default: return 'hsl(var(--foreground))';
    }
  };

  return (
    <div class="settings-section">
      {/* Sub-navigation */}
      <Tabs value={view()} onChange={(v) => { setView(v as SkillsView); if (v === 'discover' && discoveredSkills().length === 0) void discoverSkills(); }}>
        <div class="flex items-center gap-2 mb-4">
          <TabsList>
            <TabsTrigger value="installed">Installed ({installedSkills().length})</TabsTrigger>
            <TabsTrigger value="discover">Discover</TabsTrigger>
            <TabsTrigger value="sources">Sources</TabsTrigger>
          </TabsList>
          <div class="flex-1" />
          <Button variant="outline" size="sm" onClick={rebuildIndex} disabled={loading()}>
            {loading() ? <><Hourglass size={14} /> Rebuilding...</> : <><RefreshCw size={14} /> Rebuild Index</>}
          </Button>
        </div>
      <Show when={error()}>
        <div class="mb-3 px-3 py-2 rounded-md bg-destructive/15 text-destructive">
          {error()}
        </div>
      </Show>

      {/* Installed skills view */}
      <Show when={view() === 'installed'}>
        <Show when={installedSkills().length > 0} fallback={
          <EmptyState
            title="No skills installed"
            description="Use the Discover tab to find and install skills."
          />
        }>
          <div class="flex flex-col gap-2">
            <For each={installedSkills()}>
              {(skill) => (
                <article class="rounded-lg border bg-card p-3">
                  <header class="flex items-center justify-between">
                    <div>
                      <strong>{skill.manifest.name}</strong>
                      <span class="ml-2 text-xs text-muted-foreground">{skill.source_id}</span>
                    </div>
                    <div class="flex gap-1.5 items-center">
                      <label class="flex items-center gap-1 text-sm cursor-pointer">
                        <input
                          type="checkbox"
                          checked={skill.enabled}
                          onChange={(e) => void toggleSkillEnabled(skill.manifest.name, e.currentTarget.checked)}
                        />
                        Enabled
                      </label>
                      <Button variant="destructive" size="sm" onClick={() => setConfirmUninstall(skill.manifest.name)}>
                        Uninstall
                      </Button>
                    </div>
                  </header>
                  <p class="mt-1 text-sm text-muted-foreground">
                    {skill.manifest.description}
                  </p>
                  <Show when={skill.audit.risks.length > 0}>
                    <div class="mt-1.5 text-xs">
                      <Badge variant="outline" style={{ color: severityColor(skill.audit.risks.reduce((max, r) => r.severity === 'critical' ? 'critical' : r.severity === 'high' && max !== 'critical' ? 'high' : max, 'low')) }}>
                        <TriangleAlert size={14} /> {skill.audit.risks.length} risk{skill.audit.risks.length > 1 ? 's' : ''} acknowledged
                      </Badge>
                    </div>
                  </Show>
                </article>
              )}
            </For>
          </div>
        </Show>
      </Show>

      {/* Discover skills view */}
      <Show when={view() === 'discover'}>
        <Show when={discovering()}>
          <div class="flex items-center gap-2 p-4">
            <span class="spinner" /> Scanning skill sources...
          </div>
        </Show>
        <Show when={!discovering() && discoveredSkills().length > 0}>
          <div class="mb-3">
            <input
              type="text"
              placeholder="Search skills by name, description, or source..."
              value={discoverSearch()}
              onInput={(e) => setDiscoverSearch(e.currentTarget.value)}
              class="w-full"
            />
          </div>
          <Show when={filteredDiscoveredSkills().length > 0} fallback={
            <p class="text-muted-foreground text-sm">No skills match "{discoverSearch()}"</p>
          }>
            <div class="flex flex-col gap-2">
              <For each={filteredDiscoveredSkills()}>
              {(skill) => (
                <article class="rounded-lg border bg-card p-3">
                  <header class="flex items-center justify-between">
                    <div>
                      <strong>{skill.manifest.name}</strong>
                      <span class="ml-2 text-xs text-muted-foreground">{skill.source_id}</span>
                    </div>
                    <Show when={skill.installed} fallback={
                      <Button size="sm" onClick={() => startAudit(skill)}>
                        Install
                      </Button>
                    }>
                      <Badge variant="default">Installed</Badge>
                    </Show>
                  </header>
                  <p class="mt-1 text-sm text-muted-foreground">
                    {skill.manifest.description}
                  </p>
                  <Show when={skill.manifest.license}>
                    <Badge variant="outline" class="mt-1 text-xs"><ScrollText size={14} /> {skill.manifest.license}</Badge>
                  </Show>
                </article>
              )}
            </For>
          </div>
          </Show>
        </Show>
        <Show when={!discovering() && discoveredSkills().length === 0}>
          <p class="text-muted-foreground text-sm">No skills discovered. Click "Discover" to scan configured sources, or add sources in the Sources tab.</p>
        </Show>
      </Show>

      {/* Sources management view */}
      <Show when={view() === 'sources'}>
        <div class="settings-section">
          <h3>Skill Sources</h3>
          <p class="text-muted-foreground text-sm">GitHub repositories to scan for skills.</p>
          <div class="flex flex-col gap-2 mb-4">
            <For each={sources()}>
              {(source, idx) => (
                <div class="flex items-center gap-2 px-3 py-2 bg-secondary rounded-md">
                  <span class="flex-1">
                    <code>{source.owner}/{source.repo}</code>
                  </span>
                  <Button variant="destructive" size="sm" onClick={() => void removeSource(idx())}>
                    Remove
                  </Button>
                </div>
              )}
            </For>
          </div>
          <div class="flex gap-2 items-end">
            <div class="flex-1">
              <label class="text-xs text-muted-foreground">Owner</label>
              <input
                type="text"
                placeholder="e.g. anthropics"
                value={newSourceOwner()}
                onInput={(e) => setNewSourceOwner(e.currentTarget.value)}
                class="w-full"
              />
            </div>
            <div class="flex-1">
              <label class="text-xs text-muted-foreground">Repository</label>
              <input
                type="text"
                placeholder="e.g. skills"
                value={newSourceRepo()}
                onInput={(e) => setNewSourceRepo(e.currentTarget.value)}
                class="w-full"
              />
            </div>
            <Button size="sm" onClick={() => void addSource()} disabled={!newSourceOwner().trim() || !newSourceRepo().trim()}>
              Add
            </Button>
          </div>
        </div>
      </Show>
      </Tabs>

      {/* Audit dialog overlay */}
      <Dialog open={!!auditTarget()} onOpenChange={(open) => { if (!open && !auditRunning()) setAuditTarget(null); }}>
        <DialogContent
          class="max-w-[600px] !overflow-hidden !flex !flex-col"
          style="max-height: 85vh;"
          onInteractOutside={(e) => { if (auditRunning()) e.preventDefault(); }}
          onFocusOutside={(e) => e.preventDefault()}
          onEscapeKeyDown={(e) => { if (auditRunning()) e.preventDefault(); }}
        >
        <Show when={auditTarget()}>
          <DialogHeader>
            <DialogTitle class="flex items-center gap-2"><Shield size={24} /> Security Audit — {auditTarget()!.manifest.name}</DialogTitle>
          </DialogHeader>

          <div style="overflow-y: auto; overflow-x: hidden; flex: 1; min-height: 0;">
            <p class="text-sm text-muted-foreground">
              {auditTarget()!.manifest.description}
            </p>
            <p class="text-xs text-muted-foreground mb-4" style="overflow-wrap: break-word; word-break: break-all;">
              Source: <code>{auditTarget()!.source_id}</code> / <code>{auditTarget()!.source_path}</code>
            </p>

            <Show when={!auditResult()}>
              <div class="mb-4">
                <label class="text-sm font-medium block mb-1">
                  Select model for security audit:
                </label>
                <select
                  value={auditModel()}
                  onChange={(e) => setAuditModel(e.currentTarget.value)}
                  class="w-full"
                  disabled={auditRunning()}
                >
                  <option value="">— Select a model —</option>
                  <For each={props.availableModels}>
                    {(model) => <option value={model.id}>{model.label}</option>}
                  </For>
                </select>
              </div>

              <Show when={auditRunning()}>
                <div class="flex items-center gap-2 mb-3">
                  <span class="spinner" />
                  <span>{auditStatus()}</span>
                </div>
              </Show>
              <Show when={!auditRunning() && auditStatus() && !auditResult()}>
                <p class="text-sm text-destructive mb-3">{auditStatus()}</p>
              </Show>
            </Show>

            <Show when={auditResult()}>
              <div class="mb-4">
                <h4 class="mb-2">Audit Results</h4>
                <p class="mb-3 text-sm text-muted-foreground">
                  {auditResult()!.summary}
                </p>

                <Show when={auditResult()!.risks.length > 0}>
                  <div class="flex flex-col gap-2">
                    <For each={auditResult()!.risks}>
                      {(risk) => (
                        <div class="p-3 rounded-md bg-secondary" style={`border-left:3px solid ${severityColor(risk.severity)}; overflow-wrap: break-word;`}>
                          <div class="flex items-center justify-between mb-1" style="flex-wrap: wrap; gap: 0.25rem;">
                            <strong class="text-sm">{risk.id}</strong>
                            <div class="flex gap-1.5 items-center">
                              <Badge variant="outline" style={{ color: severityColor(risk.severity) }}>
                                {risk.severity.toUpperCase()}
                              </Badge>
                              <span class="text-xs text-muted-foreground">
                                {Math.round(risk.probability * 100)}% likely
                              </span>
                            </div>
                          </div>
                          <p class="text-sm text-muted-foreground">{risk.description}</p>
                          <Show when={risk.evidence}>
                            <pre class="mt-1.5 text-xs p-1.5 bg-background rounded max-h-20" style="overflow: auto; white-space: pre-wrap; word-break: break-all;">
                              {risk.evidence}
                            </pre>
                          </Show>
                        </div>
                      )}
                    </For>
                  </div>
                </Show>
                <Show when={auditResult()!.risks.length === 0}>
                  <div class="p-3 text-center text-green-400">
                    <CheckCircle size={14} /> No risks identified
                  </div>
                </Show>
              </div>

              <Show when={auditStatus()}>
                <p class="text-sm text-muted-foreground mb-2">{auditStatus()}</p>
              </Show>
            </Show>
          </div>

          <Show when={!auditResult()}>
            <DialogFooter class="flex-row gap-2" style="flex-shrink: 0;">
              <Button variant="outline" onClick={() => setAuditTarget(null)} disabled={auditRunning()}>Cancel</Button>
              <Button onClick={() => void runAudit()} disabled={!auditModel() || auditRunning()}>
                <Search size={14} /> Start Audit
              </Button>
            </DialogFooter>
          </Show>

          <Show when={auditResult()}>
            <DialogFooter class="flex-row gap-2" style="flex-shrink: 0;">
              <Button variant="outline" onClick={() => { setAuditTarget(null); setAuditResult(null); }} disabled={auditRunning()}>
                Cancel
              </Button>
              <Button onClick={() => void confirmInstall()} disabled={auditRunning()}>
                {auditResult()!.risks.length > 0 ? <><TriangleAlert size={14} /> Install Anyway</> : <><CheckCircle size={14} /> Install</>}
              </Button>
            </DialogFooter>
          </Show>
        </Show>
        </DialogContent>
      </Dialog>

      <ConfirmDialog
        open={!!confirmUninstall()}
        onOpenChange={(open) => { if (!open) setConfirmUninstall(null); }}
        title={`Uninstall "${confirmUninstall()}"?`}
        description="This skill will be removed from the persona."
        confirmLabel="Uninstall"
        variant="destructive"
        onConfirm={() => {
          const name = confirmUninstall();
          if (name) void uninstallSkill(name);
        }}
      />
    </div>
  );
};

export default SkillsTab;
