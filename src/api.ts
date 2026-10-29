// Typed bridge to the Rust backend. Every command returns a promise and every
// error arrives as a plain string that the UI can show to the user.

import { invoke } from "@tauri-apps/api/core";

export type NoteColor = "amarillo" | "rosa" | "verde" | "azul" | "morado" | "naranja";
export type NoteMode = "texto" | "lista";

export interface ChecklistItem {
  text: string;
  done: boolean;
}

export interface NoteView {
  id: string;
  color: NoteColor;
  mode: NoteMode;
  title: string;
  text: string;
  items: ChecklistItem[];
  tags: string[];
  visible: boolean;
  locked: boolean;
  unlocked: boolean;
  always_on_top: boolean;
  reminder_at: number | null;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Geometry {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Single source of truth for the paper colours. */
export const COLORS: { id: NoteColor; label: string; hex: string }[] = [
  { id: "amarillo", label: "Amarillo", hex: "#fff3b0" },
  { id: "rosa", label: "Rosa", hex: "#ffd6e2" },
  { id: "verde", label: "Verde", hex: "#d8f5c3" },
  { id: "azul", label: "Azul", hex: "#d2e8ff" },
  { id: "morado", label: "Morado", hex: "#e5d8ff" },
  { id: "naranja", label: "Naranja", hex: "#ffdfbd" },
];

export function colorOf(id: string): { label: string; hex: string } {
  const found = COLORS.find((color) => color.id === id);
  return found ?? COLORS[0];
}

export const api = {
  listNotes: () => invoke<NoteView[]>("list_notes"),
  getNote: (id: string) => invoke<NoteView>("get_note", { id }),
  createNote: () => invoke<NoteView>("create_note"),
  saveText: (id: string, text: string) => invoke<void>("save_text", { id, text }),
  saveItems: (id: string, items: ChecklistItem[]) => invoke<void>("save_items", { id, items }),
  setMode: (id: string, mode: NoteMode) => invoke<NoteView>("set_mode", { id, mode }),
  setColor: (id: string, color: NoteColor) => invoke<void>("set_color", { id, color }),
  setPin: (id: string, pin: boolean) => invoke<void>("set_pin", { id, pin }),
  saveGeometry: (id: string, geometry: Geometry) => invoke<void>("save_geometry", { id, ...geometry }),
  setReminder: (id: string, at: number | null) => invoke<void>("set_reminder", { id, at }),
  setVisible: (id: string, visible: boolean) => invoke<void>("set_visible", { id, visible }),
  deleteNote: (id: string) => invoke<void>("delete_note", { id }),
  addTag: (id: string, tag: string) => invoke<string[]>("add_tag", { id, tag }),
  removeTag: (id: string, tag: string) => invoke<string[]>("remove_tag", { id, tag }),
  lockNote: (id: string, password: string) => invoke<void>("lock_note", { id, password }),
  unlockNote: (id: string, password: string) => invoke<NoteView>("unlock_note", { id, password }),
  relockNote: (id: string) => invoke<void>("relock_note", { id }),
  removePassword: (id: string, password: string) =>
    invoke<void>("remove_password", { id, password }),
  openList: () => invoke<void>("open_list"),
  showAll: () => invoke<void>("show_all"),
  hideAll: () => invoke<void>("hide_all"),
  quit: () => invoke<void>("quit_app"),
};
