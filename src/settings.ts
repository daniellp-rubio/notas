// The settings window: general options, shortcuts, backups, export/import and
// the user-selected cloud-folder synchronization.

import { listen } from "@tauri-apps/api/event";

import { api, type SettingsState } from "./api";
import { byId, confirmDialog, describeError, el, toast } from "./ui";

let state: SettingsState;
let autostartBox: HTMLInputElement;
let lastBackupLabel: HTMLElement;
let backupSelect: HTMLSelectElement;
let shortcutsHost: HTMLElement;

export async function mountSettings(): Promise<void> {
  document.title = "Ajustes";
  document.body.classList.add("settings-mode");
  byId("app").replaceChildren(
    el("div", { class: "settings-shell", attrs: { id: "settings-shell" } }),
  );
  await load();
}

/** Build the whole window from the current state. */
async function load(): Promise<void> {
  try {
    state = await api.settingsState();
  } catch (error) {
    toast(describeError(error));
    return;
  }
  paint();
}

function paint(): void {
  byId("settings-shell").replaceChildren(
    el("header", { class: "settings-head" }, [
      el("h1", { class: "settings-title", text: "Ajustes" }),
      el("p", { class: "settings-sub", text: "Notas · versión 2.0.0" }),
    ]),
    generalSection(),
    shortcutsSection(),
    backupsSection(),
    syncSection(),
  );
  paintBackups();
}

function section(id: string, title: string, children: Node[]): HTMLElement {
  return el("section", { class: "sect", attrs: { id } }, [
    el("h2", { class: "sect-title", text: title }),
    ...children,
  ]);
}

function generalSection(): HTMLElement {
  autostartBox = el("input", {
    attrs: { type: "checkbox", id: "autostart" },
  }) as HTMLInputElement;
  autostartBox.checked = state.autostart;
  autostartBox.addEventListener("change", () => {
    autostartBox.disabled = true;
    void api
      .setAutostart(autostartBox.checked)
      .then((actual) => {
        autostartBox.checked = actual;
      })
      .catch((error) => {
        autostartBox.checked = !autostartBox.checked;
        toast(describeError(error));
      })
      .finally(() => {
        autostartBox.disabled = false;
      });
  });

  const label = el("label", { class: "check-row", attrs: { for: "autostart" } }, [
    autostartBox,
    el("span", { text: "Iniciar con Windows" }),
  ]);
  return section("general", "General", [
    label,
    el("p", { class: "sect-hint", text: "La aplicación se abre sola al iniciar sesión." }),
  ]);
}

function shortcutsSection(): HTMLElement {
  shortcutsHost = el("div", { class: "shortcut-list" });
  for (const shortcut of state.shortcuts) {
    shortcutsHost.append(
      el("div", { class: "shortcut" }, [
        el("div", { class: "shortcut-main" }, [
          el("span", { class: "shortcut-label", text: shortcut.label }),
          el("kbd", { class: "kbd", text: shortcut.accelerator }),
        ]),
        el("span", {
          class: `shortcut-state ${shortcut.active ? "ok" : "bad"}`,
          text: shortcut.active ? "Activo" : "No disponible (lo usa otro programa)",
        }),
      ]),
    );
  }
  if (state.shortcuts.length === 0) {
    shortcutsHost.append(el("p", { class: "sect-hint", text: "No hay atajos registrados." }));
  }
  return section("shortcuts", "Atajos de teclado", [
    shortcutsHost,
    el("p", { class: "sect-hint", text: "Funcionan con cualquier ventana activa." }),
  ]);
}

function formatDate(ms: number | null): string {
  if (!ms) return "Todavía no hay ninguna copia";
  const date = new Date(ms);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(date.getDate())}/${pad(date.getMonth() + 1)}/${date.getFullYear()} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function button(label: string, onClick: () => void, extra = ""): HTMLButtonElement {
  const node = el("button", {
    class: `btn${extra ? ` ${extra}` : ""}`,
    text: label,
    attrs: { type: "button" },
  });
  node.addEventListener("click", onClick);
  return node;
}

function backupsSection(): HTMLElement {
  lastBackupLabel = el("p", { class: "sect-hint" });
  backupSelect = el("select", { class: "field select", attrs: { "aria-label": "Copias disponibles" } }) as HTMLSelectElement;

  const backupNow = button("Hacer copia ahora", () => void run(backupNowAction), "primary");
  const restore = button("Restaurar copia automática…", () => void run(restoreAction));
  const folder = button("Abrir carpeta de copias", () => void run(openFolderAction));
  const markdown = button("Exportar a Markdown…", () => void run(exportMarkdownAction));
  const json = button("Exportar copia completa…", () => void run(exportJsonAction));
  const importButton = button("Importar copia…", () => void run(importAction));

  return section("backups", "Copias de seguridad", [
    lastBackupLabel,
    el("div", { class: "backup-actions" }, [
      backupNow,
      restore,
      folder,
      markdown,
      json,
      importButton,
    ]),
    el("label", { class: "sect-label", text: "Copias disponibles", attrs: { for: "backups" } }),
    backupSelect,
    el("p", {
      class: "sect-hint",
      text: "Se guarda una copia automática al día y se conservan las 7 últimas.",
    }),
  ]);
}

function syncSection(): HTMLElement {
  const sync = state.sync;
  const folder = sync.folder
    ? `Carpeta de sincronización: ${sync.folder}`
    : "Carpeta de sincronización: no elegida";
  const status = sync.error
    ? `Error: ${sync.error}`
    : `Última sincronización: ${sync.last_sync_at ? formatDate(sync.last_sync_at) : "todavía no hay ninguna"}`;
  const choose = button("Elegir carpeta…", () => void runSync(async () => {
    const picked = await api.chooseSyncFolder();
    if (picked) toast("Sincronización activada");
  }), "primary");
  const now = button("Sincronizar ahora", () => void runSync(async () => {
    const result = await api.syncNow();
    toast(`${result.synced} notas sincronizadas${result.conflicts ? ` · ${result.conflicts} conflictos` : ""}`);
  }));
  now.disabled = !sync.folder;
  const disable = button("Desactivar", () => void runSync(async () => {
    await api.disableSync();
    toast("Sincronización desactivada");
  }));
  disable.disabled = !sync.folder;
  const statusNode = el("p", {
    class: `sect-hint${sync.error ? " sync-error" : ""}`,
    text: status,
  });
  return section("sync", "Sincronización", [
    el("p", { class: "sect-hint", text: folder }),
    el("div", { class: "backup-actions" }, [choose, now, disable]),
    statusNode,
    el("p", {
      class: "sect-hint",
      text: sync.suggested_folder
        ? `Sugerencia: ${sync.suggested_folder}`
        : "Elige una carpeta privada de OneDrive, Drive u otro servicio.",
    }),
    el("p", {
      class: "sect-hint",
      text: "La carpeta guarda las notas bloqueadas cifradas y las desbloqueadas legibles en Notas.md.",
    }),
  ]);
}

function paintBackups(): void {
  lastBackupLabel.textContent = `Última copia automática: ${formatDate(state.last_backup_at)}`;
  backupSelect.replaceChildren();
  if (state.backups.length === 0) {
    backupSelect.append(el("option", { text: "Todavía no hay copias", attrs: { value: "" } }));
    backupSelect.disabled = true;
    return;
  }
  backupSelect.disabled = false;
  for (const entry of state.backups) {
    backupSelect.append(
      el("option", {
        text: `${entry.name.replace(/^notas-|\.db$/g, "")} · ${formatSize(entry.size)}`,
        attrs: { value: entry.name },
      }),
    );
  }
}

async function run(action: () => Promise<void>): Promise<void> {
  const buttons = document.querySelectorAll<HTMLButtonElement>("#backups .btn");
  buttons.forEach((node) => {
    node.disabled = true;
  });
  try {
    await action();
  } catch (error) {
    toast(describeError(error));
  } finally {
    buttons.forEach((node) => {
      node.disabled = false;
    });
    void load();
  }
}

async function runSync(action: () => Promise<void>): Promise<void> {
  const buttons = document.querySelectorAll<HTMLButtonElement>("#sync .btn");
  buttons.forEach((node) => {
    node.disabled = true;
  });
  try {
    await action();
  } catch (error) {
    toast(describeError(error));
  } finally {
    void load();
  }
}

async function backupNowAction(): Promise<void> {
  const path = await api.backupNow();
  toast(`Copia guardada: ${path.split(/[\\/]/).pop() ?? path}`);
}

async function restoreAction(): Promise<void> {
  const name = backupSelect.value;
  if (!name) {
    toast("No hay ninguna copia para restaurar");
    return;
  }
  const ok = await confirmDialog(
    "Restaurar copia automática",
    "Se reemplazarán todas tus notas actuales por las de la copia elegida. Antes se guarda una copia de seguridad de las notas actuales.",
    "Restaurar",
    true,
  );
  if (!ok) return;
  await api.restoreBackup(name);
  toast("Copia restaurada");
}

async function openFolderAction(): Promise<void> {
  await api.openBackupsFolder();
}

async function exportMarkdownAction(): Promise<void> {
  const path = await api.exportMarkdown();
  if (path) toast(`Exportado a ${path.split(/[\\/]/).pop() ?? path}`);
}

async function exportJsonAction(): Promise<void> {
  const path = await api.exportJson();
  if (path) toast(`Exportado a ${path.split(/[\\/]/).pop() ?? path}`);
}

async function importAction(): Promise<void> {
  const summary = await api.importJson();
  if (!summary) return;
  toast(
    `${summary.imported} importadas, ${summary.updated} actualizadas, ${summary.unchanged} sin cambios`,
  );
}

// The tray and this window share the autostart state; keep them in sync.
void listen<boolean>("autostart-changed", (event) => {
  if (autostartBox) autostartBox.checked = event.payload;
});
