// Small DOM helpers shared by the note window and the notes list.

export type IconName =
  | "plus"
  | "palette"
  | "pin"
  | "bell"
  | "lock"
  | "tag"
  | "dots"
  | "x"
  | "eye"
  | "eye-off"
  | "trash"
  | "list"
  | "check";

const PATHS: Record<IconName, string> = {
  plus: '<path d="M8 3.2v9.6M3.2 8h9.6"/>',
  palette:
    '<circle cx="8" cy="8" r="5.6"/><circle cx="8" cy="8" r="1.3" fill="currentColor" stroke="none"/>',
  pin: '<path d="M6 2.4h4M8 2.4v4.1l2.8 2.8v1H5.2v-1L8 6.5V2.4M8 10.3v3.3"/>',
  bell: '<path d="M8 2.2a4 4 0 0 0-4 4v2.7l-1.1 2h10.2l-1.1-2V6.2a4 4 0 0 0-4-4z"/><path d="M6.6 13.1a1.55 1.55 0 0 0 2.8 0"/>',
  lock: '<rect x="3.4" y="7" width="9.2" height="6.2" rx="1.6"/><path d="M5.6 7V5.1a2.4 2.4 0 0 1 4.8 0V7"/>',
  tag: '<path d="M2.6 8.6V3.4a.8.8 0 0 1 .8-.8h5.1l5 5-5.9 5.9-5-5z"/><circle cx="5.6" cy="5.6" r=".9" fill="currentColor" stroke="none"/>',
  dots: '<circle cx="3.6" cy="8" r="1.15" fill="currentColor" stroke="none"/><circle cx="8" cy="8" r="1.15" fill="currentColor" stroke="none"/><circle cx="12.4" cy="8" r="1.15" fill="currentColor" stroke="none"/>',
  x: '<path d="M4.4 4.4l7.2 7.2M11.6 4.4l-7.2 7.2"/>',
  eye: '<path d="M1.8 8s2.2-3.8 6.2-3.8S14.2 8 14.2 8s-2.2 3.8-6.2 3.8S1.8 8 1.8 8z"/><circle cx="8" cy="8" r="1.7"/>',
  "eye-off":
    '<path d="M2.2 2.2l11.6 11.6"/><path d="M6.1 4.5A6.7 6.7 0 0 1 8 4.2c4 0 6.2 3.8 6.2 3.8a10 10 0 0 1-1.8 2.2M4.6 5.6A9.6 9.6 0 0 0 1.8 8s2.2 3.8 6.2 3.8a6 6 0 0 0 2.3-.45"/>',
  trash:
    '<path d="M2.8 4.4h10.4M6.2 4.4V2.9h3.6v1.5M4.2 4.4l.7 8.2a.9.9 0 0 0 .9.8h4.4a.9.9 0 0 0 .9-.8l.7-8.2"/>',
  list: '<path d="M5.6 4.4h7.6M5.6 8h7.6M5.6 11.6h7.6"/><circle cx="3" cy="4.4" r=".95" fill="currentColor" stroke="none"/><circle cx="3" cy="8" r=".95" fill="currentColor" stroke="none"/><circle cx="3" cy="11.6" r=".95" fill="currentColor" stroke="none"/>',
  check: '<path d="M3.2 8.4l3 3 6.6-6.8"/>',
};

export function icon(name: IconName, size = 14): HTMLElement {
  const holder = document.createElement("span");
  holder.className = "svg";
  holder.innerHTML = `<svg width="${size}" height="${size}" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${PATHS[name]}</svg>`;
  return holder;
}

export interface ElOptions {
  class?: string;
  text?: string;
  title?: string;
  html?: string;
  attrs?: Record<string, string>;
}

export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  options: ElOptions = {},
  children: Array<Node | string> = [],
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (options.class) node.className = options.class;
  if (options.text !== undefined) node.textContent = options.text;
  if (options.title) node.title = options.title;
  if (options.html !== undefined) node.innerHTML = options.html;
  for (const [key, value] of Object.entries(options.attrs ?? {})) {
    node.setAttribute(key, value);
  }
  for (const child of children) node.append(child);
  return node;
}

export function icoButton(name: IconName, title: string, onClick: () => void): HTMLButtonElement {
  const button = el("button", { class: "ico", title, attrs: { type: "button" } }, [icon(name)]);
  button.addEventListener("click", onClick);
  return button;
}

export function byId<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`Falta el elemento #${id}`);
  return node as T;
}

export interface Debounced<A extends unknown[]> {
  (...args: A): void;
  /** Drop the pending call. */
  cancel(): void;
}

/** Trailing debounce. The returned function keeps the last arguments. */
export function debounce<A extends unknown[]>(fn: (...args: A) => void, wait: number): Debounced<A> {
  let timer: number | undefined;
  const debounced = ((...args: A) => {
    if (timer !== undefined) window.clearTimeout(timer);
    timer = window.setTimeout(() => {
      timer = undefined;
      fn(...args);
    }, wait);
  }) as Debounced<A>;
  debounced.cancel = () => {
    if (timer !== undefined) window.clearTimeout(timer);
    timer = undefined;
  };
  return debounced;
}

/** One-line error text for a rejected invoke. */
export function describeError(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return "Error inesperado";
}

// ---------------------------------------------------------------- popover

let openPopoverNode: HTMLElement | null = null;
let openPopoverAnchor: HTMLElement | null = null;

export function closePopover(): void {
  openPopoverNode?.remove();
  openPopoverNode = null;
  openPopoverAnchor = null;
}

export function openPopover(anchor: HTMLElement, build: () => HTMLElement): void {
  closePopover();
  const node = el("div", { class: "popover", attrs: { role: "dialog" } });
  node.append(build());
  document.body.append(node);

  const rect = anchor.getBoundingClientRect();
  const width = node.offsetWidth;
  const height = node.offsetHeight;
  let left = rect.right - width;
  left = Math.max(6, Math.min(left, window.innerWidth - width - 6));
  let top = rect.bottom + 4;
  if (top + height > window.innerHeight - 6) top = Math.max(6, rect.top - height - 4);
  node.style.left = `${left}px`;
  node.style.top = `${top}px`;

  openPopoverNode = node;
  openPopoverAnchor = anchor;
}

document.addEventListener("mousedown", (event) => {
  const target = event.target as Node | null;
  if (!target || !openPopoverNode) return;
  if (openPopoverNode.contains(target) || openPopoverAnchor?.contains(target)) return;
  closePopover();
});

document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && openPopoverNode) {
    event.preventDefault();
    closePopover();
  }
});

// ---------------------------------------------------------------- dialogs

interface DialogButton {
  label: string;
  primary?: boolean;
  danger?: boolean;
}

interface DialogOptions {
  title: string;
  message?: string;
  fields?: { label: string; type: "text" | "password"; placeholder?: string }[];
  buttons: DialogButton[];
}

/** Generic modal. Resolves with the field values, or null when cancelled. */
export function dialog(options: DialogOptions): Promise<string[] | null> {
  const { promise, resolve } = Promise.withResolvers<string[] | null>();
  const inputs: HTMLInputElement[] = [];
  const card = el("form", { class: "dialog" });
  card.append(el("h2", { class: "dialog-title", text: options.title }));
  if (options.message) card.append(el("p", { class: "dialog-text", text: options.message }));

  for (const field of options.fields ?? []) {
    const input = el("input", {
      class: "field",
      attrs: { type: field.type, placeholder: field.placeholder ?? "", autocomplete: "off" },
    }) as HTMLInputElement;
    inputs.push(input);
    card.append(input);
  }

  const error = el("p", { class: "dialog-error" });
  card.append(error);

  const actions = el("div", { class: "dialog-actions" });
  const close = (value: string[] | null) => {
    overlay.remove();
    document.removeEventListener("keydown", onKey);
    resolve(value);
  };
  options.buttons.forEach((spec) => {
    const button = el("button", {
      class: `btn${spec.primary ? " primary" : ""}${spec.danger ? " danger" : ""}`,
      text: spec.label,
      attrs: { type: "button" },
    });
    button.addEventListener("click", () => {
      if (!spec.primary) {
        close(null);
        return;
      }
      const values = inputs.map((input) => input.value);
      if (values.some((value) => value.trim() === "")) {
        error.textContent = "Falta un dato";
        inputs[0]?.focus();
        return;
      }
      close(values);
    });
    actions.append(button);
  });
  card.append(actions);

  const overlay = el("div", { class: "overlay" }, [card]);
  overlay.addEventListener("mousedown", (event) => {
    if (event.target === overlay) close(null);
  });
  const onKey = (event: KeyboardEvent) => {
    if (event.key === "Escape") {
      event.preventDefault();
      close(null);
    }
    if (event.key === "Enter" && document.activeElement instanceof HTMLInputElement) {
      event.preventDefault();
      actions.querySelector<HTMLButtonElement>(".btn.primary")?.click();
    }
  };
  document.addEventListener("keydown", onKey);
  card.addEventListener("submit", (event) => event.preventDefault());
  document.body.append(overlay);
  inputs[0]?.focus();
  return promise;
}

export async function confirmDialog(
  title: string,
  message: string,
  confirmLabel = "Aceptar",
  danger = false,
): Promise<boolean> {
  const values = await dialog({
    title,
    message,
    buttons: [{ label: "Cancelar" }, { label: confirmLabel, primary: true, danger }],
  });
  return values !== null;
}

export async function askPassword(
  title: string,
  message: string,
  submitLabel: string,
  withConfirmation = false,
): Promise<string | null> {
  const values = await dialog({
    title,
    message,
    fields: withConfirmation
      ? [
          { label: "Contraseña", type: "password", placeholder: "Contraseña" },
          { label: "Repetir contraseña", type: "password", placeholder: "Repite la contraseña" },
        ]
      : [{ label: "Contraseña", type: "password", placeholder: "Contraseña" }],
    buttons: [{ label: "Cancelar" }, { label: submitLabel, primary: true }],
  });
  if (!values) return null;
  if (withConfirmation && values[0] !== values[1]) {
    await confirmDialog("Las contraseñas no coinciden", "Inténtalo de nuevo.", "Entendido");
    return null;
  }
  if (values[0].trim().length < 4) {
    await confirmDialog(
      "Contraseña demasiado corta",
      "Usa al menos 4 caracteres.",
      "Entendido",
    );
    return null;
  }
  return values[0];
}

// ---------------------------------------------------------------- toast

let toastTimer: number | undefined;

export function toast(message: string): void {
  document.querySelector(".toast")?.remove();
  const node = el("div", { class: "toast", text: message });
  document.body.append(node);
  if (toastTimer !== undefined) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => node.remove(), 4200);
}

// ---------------------------------------------------------------- dates

export function formatReminder(ms: number | null): string {
  if (!ms) return "";
  const date = new Date(ms);
  const now = new Date();
  const hours = String(date.getHours()).padStart(2, "0");
  const minutes = String(date.getMinutes()).padStart(2, "0");
  if (date.toDateString() === now.toDateString()) return `${hours}:${minutes}`;
  const day = String(date.getDate()).padStart(2, "0");
  const month = String(date.getMonth() + 1).padStart(2, "0");
  return `${day}/${month} ${hours}:${minutes}`;
}

export function toDatetimeLocal(ms: number | null): string {
  if (!ms) return "";
  const date = new Date(ms);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export function fromDatetimeLocal(value: string): number | null {
  if (!value) return null;
  const time = new Date(value).getTime();
  if (Number.isNaN(time)) return null;
  return time;
}
