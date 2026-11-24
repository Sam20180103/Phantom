type FileMutations = {
  rename: (from: string, to: string) => Promise<unknown>;
  remove: (path: string) => Promise<unknown>;
};

// Preserve the destination until the source move succeeds, including across filesystems.
export async function replaceMove(
  from: string,
  to: string,
  backup: string,
  fs: FileMutations,
): Promise<string | null> {
  await fs.rename(to, backup);
  try {
    await fs.rename(from, to);
  } catch (error) {
    try {
      await fs.rename(backup, to);
    } catch (restoreError) {
      throw new Error(
        `Move failed: ${String(error)}. Original destination preserved at ${backup}; restore failed: ${String(restoreError)}`,
      );
    }
    throw error;
  }
  try {
    await fs.remove(backup);
    return null;
  } catch {
    return `Moved successfully, but the previous destination remains at ${backup}`;
  }
}
