import { createSignal } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';

// ── Types ───────────────────────────────────────────────────────────

export interface ImportPreviewItem {
  kind: 'persona' | 'workflow';
  original_id: string;
  new_id: string;
  overwrites_existing: boolean;
}

export interface ImportPreview {
  manifest: {
    format_version: number;
    name: string;
    description?: string;
    author?: string;
    created_at: string;
    personas: { id: string; path: string }[];
    workflows: { name: string; path: string }[];
  };
  target_namespace: string;
  items: ImportPreviewItem[];
  errors: string[];
  warnings: string[];
}

export interface ImportResult {
  imported_personas: { original_id: string; new_id: string; overwritten: boolean }[];
  imported_workflows: { original_id: string; new_id: string; overwritten: boolean }[];
  skipped: string[];
  errors: { item_id: string; message: string }[];
}

// ── Store ───────────────────────────────────────────────────────────

export function createAgentKitStore() {
  // Export state
  const [selectedPersonaIds, setSelectedPersonaIds] = createSignal<string[]>([]);
  const [selectedWorkflowNames, setSelectedWorkflowNames] = createSignal<string[]>([]);
  const [kitName, setKitName] = createSignal('');
  const [kitDescription, setKitDescription] = createSignal('');
  const [exporting, setExporting] = createSignal(false);
  const [exportError, setExportError] = createSignal<string | null>(null);

  // Import state
  const [importFileContent, setImportFileContent] = createSignal<string | null>(null);
  const [importFileName, setImportFileName] = createSignal<string | null>(null);
  const [targetNamespace, setTargetNamespace] = createSignal('');
  const [importPreview, setImportPreview] = createSignal<ImportPreview | null>(null);
  const [importing, setImporting] = createSignal(false);
  const [previewing, setPreviewing] = createSignal(false);
  const [importResult, setImportResult] = createSignal<ImportResult | null>(null);
  const [importError, setImportError] = createSignal<string | null>(null);
  const [selectedImportItems, setSelectedImportItems] = createSignal<Set<string>>(new Set());

  // ── Export ──────────────────────────────────────────────────────

  function togglePersona(id: string) {
    const current = selectedPersonaIds();
    if (current.includes(id)) {
      setSelectedPersonaIds(current.filter(x => x !== id));
    } else {
      setSelectedPersonaIds([...current, id]);
    }
  }

  function toggleWorkflow(name: string) {
    const current = selectedWorkflowNames();
    if (current.includes(name)) {
      setSelectedWorkflowNames(current.filter(x => x !== name));
    } else {
      setSelectedWorkflowNames([...current, name]);
    }
  }

  async function exportKit(): Promise<{ content: string; filename: string } | null> {
    setExporting(true);
    setExportError(null);
    try {
      const result = await invoke<{ content: string; filename: string }>('agent_kit_export', {
        kit_name: kitName(),
        description: kitDescription() || null,
        author: null,
        persona_ids: selectedPersonaIds(),
        workflow_names: selectedWorkflowNames(),
      });
      return result;
    } catch (e: unknown) {
      setExportError(String(e));
      return null;
    } finally {
      setExporting(false);
    }
  }

  // ── Import ─────────────────────────────────────────────────────

  async function previewImport() {
    const content = importFileContent();
    if (!content) return;

    setPreviewing(true);
    setImportError(null);
    setImportPreview(null);
    setImportResult(null);
    try {
      const preview = await invoke<ImportPreview>('agent_kit_preview', {
        content,
        target_namespace: targetNamespace(),
      });
      setImportPreview(preview);
      // Select all items by default
      setSelectedImportItems(new Set(preview.items.map(i => i.new_id)));
    } catch (e: unknown) {
      setImportError(String(e));
    } finally {
      setPreviewing(false);
    }
  }

  function toggleImportItem(newId: string) {
    const current = new Set(selectedImportItems());
    if (current.has(newId)) {
      current.delete(newId);
    } else {
      current.add(newId);
    }
    setSelectedImportItems(current);
  }

  async function applyImport() {
    const content = importFileContent();
    if (!content) return;

    setImporting(true);
    setImportError(null);
    try {
      const result = await invoke<ImportResult>('agent_kit_import', {
        content,
        target_namespace: targetNamespace(),
        selected_items: Array.from(selectedImportItems()),
      });
      setImportResult(result);
      setImportPreview(null);
    } catch (e: unknown) {
      setImportError(String(e));
    } finally {
      setImporting(false);
    }
  }

  function resetImport() {
    setImportFileContent(null);
    setImportFileName(null);
    setTargetNamespace('');
    setImportPreview(null);
    setImportResult(null);
    setImportError(null);
    setSelectedImportItems(new Set<string>());
  }

  function resetExport() {
    setSelectedPersonaIds([]);
    setSelectedWorkflowNames([]);
    setKitName('');
    setKitDescription('');
    setExportError(null);
  }

  return {
    // Export
    selectedPersonaIds,
    setSelectedPersonaIds,
    selectedWorkflowNames,
    setSelectedWorkflowNames,
    kitName,
    setKitName,
    kitDescription,
    setKitDescription,
    exporting,
    exportError,
    togglePersona,
    toggleWorkflow,
    exportKit,
    resetExport,

    // Import
    importFileContent,
    setImportFileContent,
    importFileName,
    setImportFileName,
    targetNamespace,
    setTargetNamespace,
    importPreview,
    previewing,
    importing,
    importResult,
    importError,
    selectedImportItems,
    toggleImportItem,
    previewImport,
    applyImport,
    resetImport,
  };
}

export type AgentKitStore = ReturnType<typeof createAgentKitStore>;
