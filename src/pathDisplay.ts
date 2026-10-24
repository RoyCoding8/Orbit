export function displayPath(value: string): string {
  return value
    .replace(/\\\\\?\\UNC\\/gi, "\\\\")
    .replace(/\\\\\?\\(?=[A-Za-z]:[\\/])/g, "");
}
