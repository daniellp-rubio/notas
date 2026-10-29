// The notes list: every note with its colour, title, tags and state, plus a tag
// filter and a search that only looks at notes without a password.

import { listen } from "@tauri-apps/api/event";

import { api, colorOf, type NoteView } from "./api";
import {
  byId,
  confirmDialog,
  debounce,
  describeError,
  el,
  icon,
  toast,
} from "./ui";

let notes: NoteView[] = [];
let query = "";
let tagFilter = "";
let rowsHost: HTMLElement;
let tagSelect: HTMLSelectElement;
let searchInput: HTMLInputElement;
let countLabel: HTMLElement;

export async function mountList(): Promise<void> {
  document.title = "Lista de notas";
  document.body.classList.add("list-mode");

  countLabel = el("span", { class: "muted count" });
  searchInput = el("input", {
    class: "field search",
    attrs: { type: "search", placeholder: "Buscar en notas desbloqueadas…" },
  }) as HTMLInputElement;
  searchInput.addEventListener(
    "input",
    debounce(() => {
      query = searchInput.value.trim().toLowerCase();
      render();
    }, 120),
  );

  tagSelect = el("select", { class: "field select", attrs: { "aria-label": "Filtrar por etiqueta" } }) as HTMLSelectElement;
  tagSelect.addEventListener("change", () => {
    tagFilter = tagSelect.value;
    render();
  });

  const newNote = el("button", { class: "btn primary" }, [icon("plus", 12), el("span", { text: "Nueva nota" })]);
  newNote.addEventListener("click", () => {
    api.createNote().catch((error) => toast(describeError(error)));
  });

  const showAll = el("button", { class: "btn" }, [icon("eye", 12), el("span", { text: "Mostrar todas" })]);
  showAll.addEventListener("click", () => void runAction(() => api.showAll()));

  const hideAll = el("button", { class: "btn" }, [icon("eye-off", 12), el("span", { text: "Ocultar todas" })]);
  hideAll.addEventListener("click", () => void runAction(() => api.hideAll()));

  rowsHost = el("div", { class: "rows" });

  const head = el("header", { class: "list-head" }, [
    el("div", { class: "list-title-row" }, [
      el("h1", { class: "list-title", text: "Lista de notas" }),
      countLabel,
      el("span", { class: "spacer" }),
      newNote,
      showAll,
      hideAll,
    ]),
    el("div", { class: "list-tools" }, [searchInput, tagSelect]),
  ]);

  byId("app").replaceChildren(el("div", { class: "list-shell" }, [head, rowsHost]));

  await listen<string>("notes-changed", () => void reload());
  await reload();
}

async function reload(): Promise<void> {
  try {
    notes = await api.listNotes();
  } catch (error) {
    toast(describeError(error));
    return;
  }
  renderTags();
  render();
}

async function runAction(action: () => Promise<void>): Promise<void> {
  try {
    await action();
  } catch (error) {
    toast(describeError(error));
  }
}

function renderTags(): void {
  const previous = tagFilter;
  const tags = [...new Set(notes.flatMap((note) => note.tags))].sort((a, b) =>
    a.localeCompare(b, "es", { sensitivity: "base" }),
  );
  tagSelect.replaceChildren(el("option", { text: "Todas las etiquetas", attrs: { value: "" } }));
  for (const tag of tags) {
    tagSelect.append(el("option", { text: tag, attrs: { value: tag } }));
  }
  tagFilter = tags.includes(previous) ? previous : "";
  tagSelect.value = tagFilter;
}

function visibleNotes(): NoteView[] {
  return notes.filter((note) => {
    if (tagFilter && !note.tags.some((tag) => tag.toLowerCase() === tagFilter.toLowerCase())) {
      return false;
    }
    if (!query) return true;
    if (note.locked) return false;
    const haystack = `${note.title}\n${note.text}`.toLowerCase();
    return haystack.includes(query);
  });
}

function render(): void {
  const list = visibleNotes();
  rowsHost.replaceChildren();

  countLabel.textContent =
    notes.length === 1 ? "1 nota" : `${notes.length} notas`;

  if (list.length === 0) {
    rowsHost.append(
      el("p", { class: "empty", text: notes.length === 0 ? "Todavía no hay notas." : "Ninguna nota coincide con el filtro." }),
    );
    return;
  }

  for (const note of list) rowsHost.append(row(note));
}

function row(note: NoteView): HTMLElement {
  const swatch = el("span", { class: "swatch", title: colorOf(note.color).label });
  swatch.style.background = colorOf(note.color).hex;

  const title = el("button", {
    class: "row-title",
    text: note.locked ? "🔒 Nota bloqueada" : note.title || "Nota sin título",
    attrs: { type: "button", title: "Mostrar esta nota" },
  });
  title.addEventListener("click", () => void runAction(() => api.setVisible(note.id, true)));

  const tags = el("div", { class: "row-tags" });
  for (const tag of note.tags) tags.append(el("span", { class: "chip", text: tag }));

  const state = el("span", {
    class: `row-state${note.visible ? "" : " hidden"}`,
    text: note.visible ? "Visible" : "Oculta",
  });

  const toggle = el("button", { class: "btn small" }, [
    icon(note.visible ? "eye-off" : "eye", 12),
    el("span", { text: note.visible ? "Ocultar" : "Mostrar" }),
  ]);
  toggle.addEventListener("click", () =>
    void runAction(() => api.setVisible(note.id, !note.visible)),
  );

  const remove = el("button", { class: "btn small danger", title: "Eliminar nota" }, [icon("trash", 12)]);
  remove.addEventListener("click", async () => {
    const ok = await confirmDialog(
      "Eliminar nota",
      `Se borrará «${note.locked ? "Nota bloqueada" : note.title || "Nota sin título"}» y no se puede deshacer.`,
      "Eliminar",
      true,
    );
    if (!ok) return;
    await runAction(() => api.deleteNote(note.id));
  });

  return el("div", { class: "row" }, [
    swatch,
    el("div", { class: "row-main" }, [title, tags]),
    state,
    el("div", { class: "row-actions" }, [toggle, remove]),
  ]);
}
