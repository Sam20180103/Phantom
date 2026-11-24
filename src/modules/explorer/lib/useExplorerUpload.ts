import {
  type RefObject,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { toast } from "sonner";
import { filesFrom, pickFiles, uploadFiles } from "./fileTransfer";
import { isEditablePasteTarget, pasteTargetDir } from "@/modules/explorer/lib/clipboardPaste";
import { currentWorkspaceEnv } from "@/modules/workspace";

type Options = {
  /** The explorer's outer element; paste and drop are listened for on it. */
  containerRef: RefObject<HTMLElement | null>;
  enabled?: boolean;
  rootPath: string | null;
  /** `undefined` when the path isn't a known tree entry. */
  isDir: (path: string) => boolean | undefined;
  /** Where a paste with nothing selected should land. */
  selectedPath: string | null;
  onUploaded: (destDir: string) => void;
};

/**
 * Uploading files into the tree from the browser: the context menu, a paste,
 * and a drag from the desktop.
 *
 * The Tauri build has `useExplorerFileDrop`, which copies server-side from a
 * path the OS handed it. In a browser there is no such path — only the bytes —
 * so they go over HTTP instead.
 */
export function useExplorerUpload({
  containerRef,
  enabled = true,
  rootPath,
  isDir,
  selectedPath,
  onUploaded,
}: Options) {
  const [dropTargetDir, setDropTargetDir] = useState<string | null>(null);

  const upload = useCallback(
    async (destDir: string, files: File[], workspace = currentWorkspaceEnv()) => {
      if (files.length === 0) return;
      const label =
        files.length === 1 ? files[0].name : `${files.length} files`;
      const id = toast.loading(`Uploading ${label}…`);
      try {
        const results = await uploadFiles(destDir, files, workspace);
        toast.success(
          results.length === 1
            ? `Uploaded ${results[0].name}`
            : `Uploaded ${results.length} files`,
          { id },
        );
        onUploaded(destDir);
      } catch (e) {
        toast.error(`Upload failed: ${String(e)}`, { id });
      }
    },
    [onUploaded],
  );

  /** Menu entry point: pick files, then drop them in `destDir`. */
  const pickAndUpload = useCallback(
    async (destDir: string) => {
      const root = stateRef.current.rootPath;
      const workspace = currentWorkspaceEnv();
      if (!root || !stateRef.current.enabled) return;
      const files = await pickFiles();
      if (!stateRef.current.enabled || stateRef.current.rootPath !== root || currentWorkspaceEnv() !== workspace) return;
      await upload(destDir, files, workspace);
    },
    [upload],
  );

  // Read through a ref so the listeners below can stay mounted for the life of
  // the explorer instead of being torn down on every selection change.
  const stateRef = useRef({ rootPath, isDir, selectedPath, upload, enabled });
  stateRef.current = { rootPath, isDir, selectedPath, upload, enabled };

  /** The directory an event landed on: the row under it, else the selection. */
  const dirFor = useCallback((target: EventTarget | null): string | null => {
    const { rootPath, isDir, selectedPath } = stateRef.current;
    if (!rootPath) return null;
    const row =
      target instanceof Element
        ? target.closest<HTMLElement>("[data-fs-path]")
        : null;
    const path = row?.getAttribute("data-fs-path") ?? selectedPath;
    return pasteTargetDir(path, isDir, rootPath);
  }, []);

  useEffect(() => {
    if (!enabled) return;
    const element = containerRef.current;
    if (!element) return;

    const onPaste = (event: ClipboardEvent) => {
      if (!stateRef.current.enabled || isEditablePasteTarget(event.target)) return;
      const files = filesFrom(event.clipboardData);
      if (files.length === 0) return;
      event.preventDefault();
      const dir = dirFor(event.target);
      if (dir) void stateRef.current.upload(dir, files);
    };

    const onDragOver = (event: DragEvent) => {
      if (!stateRef.current.enabled) return;
      // Without this the browser navigates away to the dropped file.
      event.preventDefault();
      setDropTargetDir(dirFor(event.target));
    };

    const onDragLeave = (event: DragEvent) => {
      if (element.contains(event.relatedTarget as Node | null)) return;
      setDropTargetDir(null);
    };

    const onDrop = (event: DragEvent) => {
      if (!stateRef.current.enabled) return;
      setDropTargetDir(null);
      const files = filesFrom(event.dataTransfer);
      if (files.length === 0) return;
      event.preventDefault();
      const dir = dirFor(event.target);
      if (dir) void stateRef.current.upload(dir, files);
    };

    element.addEventListener("paste", onPaste);
    element.addEventListener("dragover", onDragOver);
    element.addEventListener("dragleave", onDragLeave);
    element.addEventListener("drop", onDrop);
    return () => {
      setDropTargetDir(null);
      element.removeEventListener("paste", onPaste);
      element.removeEventListener("dragover", onDragOver);
      element.removeEventListener("dragleave", onDragLeave);
      element.removeEventListener("drop", onDrop);
    };
  }, [enabled, containerRef, dirFor]);

  return { upload, pickAndUpload, dropTargetDir };
}
