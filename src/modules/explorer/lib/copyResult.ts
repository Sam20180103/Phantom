export type CopyResult = {
  created: string[];
  failed: { source: string; message: string }[];
};

export function copyFailureNotice(result: CopyResult) {
  if (result.failed.length === 0) return null;
  const lines = result.failed
    .slice(0, 3)
    .map(({ source, message }) => `${source}: ${message}`);
  if (result.failed.length > 3) lines.push(`And ${result.failed.length - 3} more`);
  return {
    message:
      result.created.length > 0 ? "Some items could not be copied" : "Copy failed",
    description: lines.join("\n"),
  };
}
