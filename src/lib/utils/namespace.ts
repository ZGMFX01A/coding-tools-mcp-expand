export function normalizeServerName(name: string): string {
  const lower = name.trim().toLowerCase();
  let normalized = "";
  let lastWasSep = false;

  for (const ch of lower) {
    if (/[a-z0-9_-]/.test(ch)) {
      normalized += ch;
      lastWasSep = false;
    } else {
      if (!lastWasSep) {
        normalized += "-";
        lastWasSep = true;
      }
    }
  }

  const trimmed = normalized.replace(/^[-_]+|[-_]+$/g, "");
  return trimmed || "mcp";
}

export function makePublicToolName(serverName: string, toolName: string): string {
  const norm = normalizeServerName(serverName);
  return `${norm}__${toolName}`;
}
