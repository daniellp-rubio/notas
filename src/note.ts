// One sticky note = one window. Everything that leaves the process goes through
// the typed API in ./api.

import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import {
  api,
  COLORS,
  colorOf,
  type ChecklistItem,
  type NoteColor,
  type NoteMode,
  type NoteView,
} from "./api";
import {
  askPassword,
  byId,
  closePopover,
  confirmDialog,
  debounce,
  describeError,
  el,
  formatReminder,
  fromDatetimeLocal,
  icon,
  icoButton,
  openPopover,
  toDatetimeLocal,
  toast,
} from "./ui";

const SAVE_DELAY = 500;

interface Chrome {
  color: HTMLButtonElement;
  pin: HTMLButtonElement;
  bell: HTMLButtonElement;
  badge: HTMLSpanElement;
  lock: HTMLButtonElement;
  tag: HTMLButtonElement;
  menu: HTMLButtonElement;
  body: HTMLElement;
}

let note: NoteView;
let chrome: Chrome;
let editor: HTMLTextAreaElement | null = null;
let itemsHost: HTMLElement | null = null;

const saveTextSoon = debounce(() => void persistText(), SAVE_DELAY);
const saveItemsSoon = debounce(() => void persistItems(), SAVE_DELAY);

function showError(error: unknown): void {
  toast(describeError(error));
}

async function persistText(): Promise<void> {
  if (!editor) return;
  note.text = editor.value;
  try {
    await api.saveText(note.id, note.text);
  } catch (error) {
    showError(error);
  }
}

async function persistItems(): Promise<void> {
  if (note.mode !== "lista") return;
  try {
    await api.saveItems(note.id, note.items);
  } catch (error) {
    showError(error);
  }
}

/** Write whatever is on screen right now, before a structural change. */
async function flushNow(): Promise<void> {
  saveTextSoon.cancel();
  saveItemsSoon.cancel();
  await persistText();
  await persistItems();
}

export async function mountNote(id: string): Promise<void> {
  try {
    note = await api.getNote(id);
  } catch (error) {
    renderFatal(describeError(error));
    return;
  }
  document.title = note.title || "Notas";
  buildShell();
  paint();
  renderBody();
  wireGeometry(id);

  await listen<string>("notes-changed", (event) => {
    if (event.payload === id) void refresh();
  });
  await listen<string>("reminder-fired", (event) => {
    if (event.payload === id) void refresh();
  });
}

// ------------------------------------------------------------------ shell

function buildShell(): void {
  const newButton = icoButton("plus", "Nueva nota", () => {
    api.createNote().catch(showError);
  });
  const colorButton = icoButton("palette", "Color", () => toggleColor());
  const pinButton = icoButton("pin", "Fijar encima de todo", () => void togglePin());
  const badge = el("span", { class: "bell-badge" });
  const bellButton = icoButton("bell", "Recordatorio", () => toggleReminder());
  bellButton.append(badge);
  const lockButton = icoButton("lock", "Contraseña", () => toggleLock());
  const tagButton = icoButton("tag", "Etiquetas", () => toggleTags());
  const menuButton = icoButton("dots", "Menú", () => toggleMenu());

  const bar = el("header", { class: "bar", attrs: { "data-tauri-drag-region": "" } }, [
    newButton,
    el("span", { class: "bar-fill", attrs: { "data-tauri-drag-region": "" } }),
    colorButton,
    pinButton,
    bellButton,
    lockButton,
    tagButton,
    menuButton,
  ]);
  const body = el("section", { class: "body" });
  byId("app").replaceChildren(el("div", { class: "note" }, [bar, body]));

  chrome = {
    color: colorButton,
    pin: pinButton,
    bell: bellButton,
    badge,
    lock: lockButton,
    tag: tagButton,
    menu: menuButton,
    body,
  };
}

/** Header state that can change without rebuilding the body. */
function paint(): void {
  const paper = colorOf(note.color).hex;
  document.documentElement.style.setProperty("--paper", paper);

  const locked = note.locked && !note.unlocked;
  chrome.color.disabled = locked;
  chrome.pin.disabled = locked;
  chrome.bell.disabled = locked;
  chrome.lock.disabled = locked;
  chrome.tag.disabled = locked;

  chrome.pin.classList.toggle("active", note.always_on_top);
  chrome.pin.title = note.always_on_top ? "Quitar de encima de todo" : "Fijar encima de todo";

  const time = formatReminder(note.reminder_at);
  chrome.badge.textContent = time;
  chrome.bell.classList.toggle("with-badge", time !== "");
  chrome.bell.classList.toggle("active", time !== "");
  chrome.bell.title = time ? `Recordatorio: ${time}` : "Recordatorio";

  chrome.lock.classList.toggle("active", note.locked);
  chrome.tag.classList.toggle("active", note.tags.length > 0 && !locked);
}

function renderFatal(message: string): void {
  byId("app").replaceChildren(
    el("div", { class: "note" }, [
      el("section", { class: "body fatal" }, [
        el("h2", { text: "No se pudo abrir la nota" }),
        el("p", { class: "muted", text: message }),
      ]),
    ]),
  );
}

function renderBody(): void {
  editor = null;
  itemsHost = null;
  closePopover();
  if (note.locked && !note.unlocked) {
    chrome.body.replaceChildren(lockPanel());
    return;
  }
  chrome.body.replaceChildren(note.mode === "lista" ? checklistView() : editorView());
}

// ---------------------------------------------------------------- content

function editorView(): HTMLElement {
  const area = el("textarea", {
    class: "editor",
    attrs: { placeholder: "Escribe aquí…", spellcheck: "false" },
  }) as HTMLTextAreaElement;
  area.value = note.text;
  area.addEventListener("input", () => {
    note.text = area.value;
    saveTextSoon();
  });
  area.addEventListener("blur", () => {
    saveTextSoon.cancel();
    void persistText();
  });
  editor = area;
  if (note.text === "") requestAnimationFrame(() => area.focus());
  return area;
}

function checklistView(): HTMLElement {
  const host = el("div", { class: "items" });
  note.items.forEach((item, index) => host.append(itemRow(item, index)));
  if (note.items.length === 0) {
    host.append(el("p", { class: "hint", text: "Sin elementos todavía." }));
  }
  const add = el("button", { class: "add-item", attrs: { type: "button" } }, [
    icon("plus", 12),
    el("span", { text: "Añadir elemento" }),
  ]);
  add.addEventListener("click", () => {
    note.items.push({ text: "", done: false });
    refreshChecklist(note.items.length - 1);
    saveItemsSoon();
  });
  host.append(add);
  itemsHost = host;
  return host;
}

function itemRow(item: ChecklistItem, index: number): HTMLElement {
  const check = el("button", {
    class: `chk${item.done ? " done" : ""}`,
    title: item.done ? "Marcar como pendiente" : "Marcar como hecho",
    attrs: { type: "button" },
  }, [el("span", { class: "chk-box" })]);

  const input = el("input", {
    class: `item-text${item.done ? " done" : ""}`,
    attrs: { type: "text", placeholder: "Elemento", spellcheck: "false" },
  }) as HTMLInputElement;
  input.value = item.text;

  check.addEventListener("click", () => {
    item.done = !item.done;
    check.classList.toggle("done", item.done);
    input.classList.toggle("done", item.done);
    saveItemsSoon();
  });

  input.addEventListener("input", () => {
    item.text = input.value;
    saveItemsSoon();
  });

  input.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      note.items.splice(index + 1, 0, { text: "", done: false });
      refreshChecklist(index + 1);
      saveItemsSoon();
      return;
    }
    if (event.key === "Backspace" && input.value === "" && note.items.length > 1) {
      event.preventDefault();
      note.items.splice(index, 1);
      refreshChecklist(Math.max(0, index - 1));
      saveItemsSoon();
      return;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      focusItem(index + (event.key === "ArrowDown" ? 1 : -1));
    }
  });

  input.addEventListener("blur", () => {
    saveItemsSoon.cancel();
    void persistItems();
  });

  const remove = el("button", {
    class: "item-del",
    title: "Eliminar elemento",
    attrs: { type: "button" },
  }, [icon("x", 12)]);
  remove.addEventListener("click", () => {
    note.items.splice(index, 1);
    refreshChecklist(Math.max(0, index - 1));
    saveItemsSoon();
  });

  return el("div", { class: "item" }, [check, input, remove]);
}

function refreshChecklist(focusIndex?: number): void {
  const scroll = itemsHost?.scrollTop ?? 0;
  chrome.body.replaceChildren(checklistView());
  if (itemsHost) itemsHost.scrollTop = scroll;
  if (focusIndex !== undefined) focusItem(focusIndex);
}

function focusItem(index: number): void {
  const inputs = itemsHost?.querySelectorAll<HTMLInputElement>("input.item-text");
  if (!inputs || inputs.length === 0) return;
  const target = inputs[Math.max(0, Math.min(index, inputs.length - 1))];
  target.focus();
  target.setSelectionRange(target.value.length, target.value.length);
}

// ------------------------------------------------------------------- lock

function lockPanel(): HTMLElement {
  const input = el("input", {
    class: "field",
    attrs: { type: "password", placeholder: "Contraseña", autocomplete: "current-password" },
  }) as HTMLInputElement;
  const button = el("button", { class: "btn primary", text: "Desbloquear", attrs: { type: "button" } });
  const error = el("p", { class: "dialog-error" });

  const submit = async () => {
    if (!input.value) {
      error.textContent = "Escribe la contraseña.";
      return;
    }
    button.disabled = true;
    try {
      note = await api.unlockNote(note.id, input.value);
      document.title = note.title || "Notas";
      paint();
      renderBody();
    } catch (failure) {
      error.textContent = describeError(failure);
      input.select();
    } finally {
      button.disabled = false;
    }
  };

  button.addEventListener("click", () => void submit());
  input.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      void submit();
    }
  });

  const panel = el("div", { class: "lock" }, [
    el("div", { class: "lock-icon" }, [icon("lock", 26)]),
    el("h2", { text: "Nota bloqueada" }),
    el("p", { class: "muted", text: "Escribe la contraseña para ver y editar la nota." }),
    input,
    button,
    error,
    el("p", { class: "hint", text: "Si olvidas la contraseña, el contenido no se puede recuperar." }),
  ]);
  requestAnimationFrame(() => input.focus());
  return panel;
}

function toggleLock(): void {
  if (note.locked) {
    openPopover(chrome.lock, () => {
      const body = el("div", { class: "pop-body" });
      body.append(el("p", { class: "pop-title", text: "Contraseña" }));
      body.append(
        el("p", {
          class: "pop-note",
          text: "La nota está cifrada en el disco con AES-256-GCM.",
        }),
      );
      const relock = el("button", { class: "menu-item", text: "Bloquear de nuevo", attrs: { type: "button" } });
      relock.addEventListener("click", () => {
        closePopover();
        void relockNote();
      });
      const remove = el("button", {
        class: "menu-item danger",
        text: "Quitar contraseña",
        attrs: { type: "button" },
      });
      remove.addEventListener("click", () => {
        closePopover();
        void removePassword();
      });
      body.append(relock, remove);
      return body;
    });
    return;
  }
  void addPassword();
}

async function addPassword(): Promise<void> {
  const password = await askPassword(
    "Bloquear con contraseña",
    "Si olvidas la contraseña, el contenido no se podrá recuperar.",
    "Bloquear",
    true,
  );
  if (!password) return;
  await flushNow();
  try {
    await api.lockNote(note.id, password);
    note.locked = true;
    note.unlocked = true;
    paint();
    toast("Nota bloqueada con contraseña");
  } catch (error) {
    showError(error);
  }
}

async function relockNote(): Promise<void> {
  await flushNow();
  try {
    await api.relockNote(note.id);
    note.unlocked = false;
    note.text = "";
    note.items = [];
    document.title = "Notas";
    paint();
    renderBody();
    toast("Nota bloqueada");
  } catch (error) {
    showError(error);
  }
}

async function removePassword(): Promise<void> {
  const password = await askPassword(
    "Quitar contraseña",
    "El contenido volverá a guardarse sin cifrar.",
    "Quitar contraseña",
  );
  if (!password) return;
  try {
    await api.removePassword(note.id, password);
    note.locked = false;
    note.unlocked = false;
    paint();
    toast("Contraseña eliminada");
  } catch (error) {
    showError(error);
  }
}

// ---------------------------------------------------------------- popover

function toggleColor(): void {
  openPopover(chrome.color, () => {
    const grid = el("div", { class: "swatches" });
    for (const color of COLORS) {
      const swatch = el("button", {
        class: `swatch${color.id === note.color ? " selected" : ""}`,
        title: color.label,
        attrs: { type: "button", "aria-label": color.label },
      });
      swatch.style.background = color.hex;
      swatch.addEventListener("click", () => {
        closePopover();
        void applyColor(color.id);
      });
      grid.append(swatch);
    }
    const body = el("div", { class: "pop-body" });
    body.append(el("p", { class: "pop-title", text: "Color" }), grid);
    return body;
  });
}

async function applyColor(color: NoteColor): Promise<void> {
  try {
    await api.setColor(note.id, color);
    note.color = color;
    paint();
  } catch (error) {
    showError(error);
  }
}

async function togglePin(): Promise<void> {
  const next = !note.always_on_top;
  try {
    await api.setPin(note.id, next);
    note.always_on_top = next;
    paint();
  } catch (error) {
    showError(error);
  }
}

function toggleReminder(): void {
  openPopover(chrome.bell, () => {
    const input = el("input", {
      class: "field",
      attrs: { type: "datetime-local" },
    }) as HTMLInputElement;
    input.value = toDatetimeLocal(note.reminder_at);

    const save = el("button", { class: "btn primary", text: "Guardar", attrs: { type: "button" } });
    const clear = el("button", { class: "btn", text: "Quitar", attrs: { type: "button" } });

    save.addEventListener("click", async () => {
      const at = fromDatetimeLocal(input.value);
      if (at === null) {
        toast("Elige una fecha y una hora");
        return;
      }
      if (at <= Date.now()) {
        toast("Esa hora ya pasó");
        return;
      }
      closePopover();
      try {
        await api.setReminder(note.id, at);
        note.reminder_at = at;
        paint();
        toast(`Recordatorio para ${formatReminder(at)}`);
      } catch (error) {
        showError(error);
      }
    });

    clear.addEventListener("click", async () => {
      closePopover();
      try {
        await api.setReminder(note.id, null);
        note.reminder_at = null;
        paint();
      } catch (error) {
        showError(error);
      }
    });

    const body = el("div", { class: "pop-body" });
    body.append(el("p", { class: "pop-title", text: "Recordatorio" }), input);
    body.append(
      el("p", { class: "pop-note", text: "Se mostrará un aviso aunque la nota esté oculta." }),
    );
    body.append(el("div", { class: "pop-actions" }, [clear, save]));
    return body;
  });
}

function toggleTags(): void {
  openPopover(chrome.tag, () => {
    const list = el("div", { class: "tag-list" });
    const draw = (tags: string[]) => {
      list.replaceChildren();
      if (tags.length === 0) {
        list.append(el("p", { class: "hint", text: "Sin etiquetas" }));
        return;
      }
      for (const tag of tags) {
        const remove = el("button", {
          class: "chip-x",
          title: "Quitar etiqueta",
          attrs: { type: "button" },
        }, [icon("x", 10)]);
        remove.addEventListener("click", async () => {
          try {
            note.tags = await api.removeTag(note.id, tag);
            draw(note.tags);
            paint();
          } catch (error) {
            showError(error);
          }
        });
        list.append(el("span", { class: "chip" }, [el("span", { text: tag }), remove]));
      }
    };
    draw(note.tags);

    const input = el("input", {
      class: "field",
      attrs: { type: "text", placeholder: "Nueva etiqueta" },
    }) as HTMLInputElement;
    const add = el("button", { class: "btn", text: "Añadir", attrs: { type: "button" } });

    const commit = async () => {
      const value = input.value.trim();
      if (!value) return;
      try {
        note.tags = await api.addTag(note.id, value);
        input.value = "";
        input.focus();
        draw(note.tags);
        paint();
      } catch (error) {
        showError(error);
      }
    };
    add.addEventListener("click", () => void commit());
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        void commit();
      }
    });

    const body = el("div", { class: "pop-body" });
    body.append(el("p", { class: "pop-title", text: "Etiquetas" }), list);
    body.append(el("div", { class: "pop-row" }, [input, add]));
    requestAnimationFrame(() => input.focus());
    return body;
  });
}

function toggleMenu(): void {
  const lockedAtRest = note.locked && !note.unlocked;
  openPopover(chrome.menu, () => {
    const body = el("div", { class: "pop-body" });
    body.append(el("p", { class: "pop-title", text: "Nota" }));

    if (!lockedAtRest) {
      const mode = el("button", {
        class: "menu-item",
        text: note.mode === "texto" ? "Convertir en lista" : "Convertir en texto",
        attrs: { type: "button" },
      });
      mode.addEventListener("click", () => {
        closePopover();
        void switchMode();
      });
      body.append(mode);
    }

    const hide = el("button", { class: "menu-item", text: "Ocultar", attrs: { type: "button" } });
    hide.addEventListener("click", () => {
      closePopover();
      void hideNote();
    });
    body.append(hide);

    const list = el("button", { class: "menu-item", text: "Lista de notas", attrs: { type: "button" } });
    list.addEventListener("click", () => {
      closePopover();
      api.openList().catch(showError);
    });
    body.append(list);

    const settings = el("button", { class: "menu-item", text: "Ajustes…", attrs: { type: "button" } });
    settings.addEventListener("click", () => {
      closePopover();
      api.openSettings().catch(showError);
    });
    body.append(settings);

    const remove = el("button", {
      class: "menu-item danger",
      text: "Eliminar",
      attrs: { type: "button" },
    });
    remove.addEventListener("click", () => {
      closePopover();
      void deleteNote();
    });
    body.append(remove);
    return body;
  });
}

// ----------------------------------------------------------------- actions

async function switchMode(): Promise<void> {
  const target: NoteMode = note.mode === "texto" ? "lista" : "texto";
  if (note.mode === "lista") {
    const ok = await confirmDialog(
      "Convertir en texto",
      "Los elementos se convertirán en líneas y se perderá la marca de hecho o pendiente. ¿Continuar?",
      "Convertir",
    );
    if (!ok) return;
  }
  await flushNow();
  try {
    note = await api.setMode(note.id, target);
    document.title = note.title || "Notas";
    paint();
    renderBody();
  } catch (error) {
    showError(error);
  }
}

async function hideNote(): Promise<void> {
  await flushNow();
  try {
    await api.setVisible(note.id, false);
  } catch (error) {
    showError(error);
  }
}

async function deleteNote(): Promise<void> {
  const ok = await confirmDialog(
    "Eliminar nota",
    "Se borrará la nota y no se puede deshacer.",
    "Eliminar",
    true,
  );
  if (!ok) return;
  saveTextSoon.cancel();
  saveItemsSoon.cancel();
  try {
    await api.deleteNote(note.id);
  } catch (error) {
    showError(error);
  }
}

// ---------------------------------------------------------------- geometry

function wireGeometry(id: string): void {
  const win = getCurrentWindow();
  const save = debounce(async () => {
    try {
      const [position, size, scale] = await Promise.all([
        win.outerPosition(),
        win.outerSize(),
        win.scaleFactor(),
      ]);
      await api.saveGeometry(id, {
        x: position.x / scale,
        y: position.y / scale,
        width: size.width / scale,
        height: size.height / scale,
      });
    } catch {
      // The window may be closing; geometry is best effort.
    }
  }, SAVE_DELAY);

  win.onMoved(() => save()).catch(() => undefined);
  win.onResized(() => save()).catch(() => undefined);
}

// ---------------------------------------------------------------- refresh

async function refresh(): Promise<void> {
  try {
    const next = await api.getNote(note.id);
    const structural =
      next.mode !== note.mode || next.locked !== note.locked || next.unlocked !== note.unlocked;
    note.color = next.color;
    note.always_on_top = next.always_on_top;
    note.tags = next.tags;
    note.reminder_at = next.reminder_at;
    note.visible = next.visible;
    note.title = next.title;
    if (structural) {
      note.mode = next.mode;
      note.locked = next.locked;
      note.unlocked = next.unlocked;
      note.text = next.text;
      note.items = next.items;
      renderBody();
    }
    paint();
  } catch {
    // The note was deleted or the window is going away: nothing to do.
  }
}
