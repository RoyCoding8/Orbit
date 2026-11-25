export function displayPath(value: string): string {
  return value
    .replace(/\\\\\?\\UNC\\/gi, "\\\\")
    .replace(/\\\\\?\\(?=[A-Za-z]:[\\/])/g, "");
}

export function normalizePathIdentity(value: string): string {
  const path = displayPath(value.trim());
  if (!path) return "";

  const isWindowsPath =
    /^[A-Za-z]:[\\/]/.test(path) ||
    path.startsWith("\\\\") ||
    path.startsWith("//");
  if (isWindowsPath) {
    const slashes = path.replace(/\\/g, "/");
    const normalized = slashes.startsWith("//")
      ? `//${slashes.slice(2).replace(/\/{2,}/g, "/")}`
      : slashes.replace(/\/{2,}/g, "/");
    return normalized.replace(/\/+$/, "").toLowerCase();
  }

  if (path.startsWith("/")) {
    return path.replace(/\/{2,}/g, "/").replace(/\/+$/, "") || "/";
  }

  return path;
}
