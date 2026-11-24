/**
 * Pasting and dropping images into a terminal.
 *
 * A terminal can't take an image — but an AI agent running in one can read a
 * file. So a pasted screenshot is uploaded to the server, and what actually
 * lands in the shell is its path.
 *
 * Requires a secure context: `navigator.clipboard` and the `paste` event's
 * image data are withheld on a plain-HTTP origin.
 */

export type PastedBlob = { path: string };

/** Uploads raw bytes and returns the server-side path to paste. */
async function upload(blob: Blob): Promise<string> {
  const response = await fetch("/blob", {
    method: "POST",
    // Same-origin, so the session cookie rides along automatically.
    headers: { "Content-Type": blob.type || "application/octet-stream" },
    body: blob,
  });
  if (!response.ok) {
    throw new Error(`upload failed: ${response.status} ${await response.text()}`);
  }
  const result = (await response.json()) as PastedBlob;
  return result.path;
}

/**
 * Shell-quotes a path so spaces and quotes survive the paste.
 *
 * Single quotes are the only fully safe form in POSIX shells: everything inside
 * is literal, and an embedded `'` is closed, escaped, and reopened.
 */
function shellQuote(path: string): string {
  // `split`/`join` rather than `replaceAll`: the tsconfig target predates it.
  return `'${path.split("'").join(`'\\''`)}'`;
}

/** Pulls image/file blobs out of a clipboard or drag payload. */
function blobsFrom(data: DataTransfer | null): Blob[] {
  if (!data) return [];
  const out: Blob[] = [];
  for (const item of data.items) {
    if (item.kind !== "file") continue;
    const file = item.getAsFile();
    if (file) out.push(file);
  }
  return out;
}

export type PasteTarget = (text: string) => void;

/**
 * Whether the event landed in a subtree marked `data-blob-paste-ignore`, which
 * handles dropped and pasted files itself (the file explorer uploads them).
 *
 * The listeners below sit on `document` in the capture phase, so they see an
 * event before anything nested does; without an opt-out the explorer could
 * never receive a paste of its own — the path would go to the terminal instead.
 */
function handledElsewhere(event: Event): boolean {
  const target = event.target;
  if (!(target instanceof Element)) return false;
  return target.closest("[data-blob-paste-ignore]") !== null;
}

/**
 * Wires paste and drop on `element` to upload-then-paste-the-path.
 *
 * Returns a disposer. Text pastes are left alone — the terminal already
 * handles those, and intercepting them would break ordinary copy/paste.
 */
export function attachBlobPaste(
  element: HTMLElement | Document,
  paste: PasteTarget,
): () => void {
  const handle = async (blobs: Blob[]) => {
    if (blobs.length === 0) return;
    try {
      const paths = await Promise.all(blobs.map(upload));
      paste(paths.map(shellQuote).join(" "));
    } catch (e) {
      console.warn("phantom: pasting a file into the terminal failed:", e);
    }
  };

  const onPaste = (event: Event) => {
    if (handledElsewhere(event)) return;
    const data = (event as ClipboardEvent).clipboardData;
    const blobs = blobsFrom(data);
    if (blobs.length === 0) {
      // Plain text: let the terminal have it. Logged because "nothing
      // happened" on an image paste is otherwise impossible to tell apart
      // from the event never arriving at all.
      console.log(
        "phantom: paste carried no files; types =",
        data ? Array.from(data.items).map((i) => `${i.kind}/${i.type}`) : "none",
      );
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    void handle(blobs);
  };

  const onDrop = (event: Event) => {
    if (handledElsewhere(event)) return;
    const blobs = blobsFrom((event as DragEvent).dataTransfer);
    if (blobs.length === 0) return;
    event.preventDefault();
    event.stopPropagation();
    void handle(blobs);
  };

  // Without this the browser navigates away to the dropped file.
  const onDragOver = (event: Event) => {
    if (handledElsewhere(event)) return;
    event.preventDefault();
  };

  // Capture phase, deliberately: xterm.js handles `paste` on its own hidden
  // textarea and stops it there, so a bubble-phase listener on `document`
  // never sees an image paste into a focused terminal.
  const capture = true;
  element.addEventListener("paste", onPaste, capture);
  element.addEventListener("drop", onDrop, capture);
  element.addEventListener("dragover", onDragOver, capture);

  return () => {
    element.removeEventListener("paste", onPaste, capture);
    element.removeEventListener("drop", onDrop, capture);
    element.removeEventListener("dragover", onDragOver, capture);
  };
}
