import { useEffect } from "react";
import { attachBlobPaste } from "@/lib/transport/blobPaste";
import { pasteIntoSession } from "./terminalSessionApi";

/**
 * Lets a screenshot be pasted or dropped into the terminal.
 *
 * A terminal has no way to accept an image, but an agent running inside one can
 * read a file — so the bytes go to the server and the shell receives the path.
 *
 * The web counterpart to `useTerminalFileDrop`, which relies on Tauri's native
 * drop events and never fires in a browser.
 */
export function useTerminalBlobPaste(activeLeafId: number | null): void {
  useEffect(() => {
    if (activeLeafId === null) {
      console.log("phantom: blob paste idle — no active terminal leaf");
      return;
    }
    console.log("phantom: blob paste armed for leaf", activeLeafId);
    return attachBlobPaste(document, (text) => {
      // Bracketed paste, so a shell treats a multi-word path as literal input
      // instead of running it.
      pasteIntoSession(activeLeafId, text);
    });
  }, [activeLeafId]);
}
