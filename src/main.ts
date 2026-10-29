import "./styles.css";

import { mountList } from "./list";
import { mountNote } from "./note";
import { mountSettings } from "./settings";
import { toast } from "./ui";

// Both windows load this entry point; the query string says which one it is.
// Rust builds `index.html?note=<id>` for a note and `index.html?view=list` for
// the manager window.
const params = new URLSearchParams(window.location.search);
const noteId = params.get("note");
const view = params.get("view");

if (noteId) {
  void mountNote(noteId);
} else if (view === "settings") {
  void mountSettings();
} else {
  void mountList().catch((error: unknown) => {
    toast(error instanceof Error ? error.message : String(error));
  });
}
