export type MemorySection = {
  title: string;
  start: number;
  end: number;
  projects: string[];
};

type Line = {
  start: number;
  end: number;
  text: string;
};

type Heading = {
  depth: number;
  title: string;
  start: number;
};

type ScannedMarkdown = {
  headings: Heading[];
  visibleLines: Line[];
};

const headingPattern = /^ {0,3}(#{1,6})(?:[ \t]+(.*)|[ \t]*)$/;
const fencePattern = /^ {0,3}(`{3,}|~{3,})(.*)$/;

export function parseMemorySections(content: string): MemorySection[] {
  if (content.length === 0) return [];

  const { headings, visibleLines } = scanMarkdown(content);
  const projectNames = new Map<string, string>();
  const splitDepth = chooseSplitDepth(headings);
  if (splitDepth === null) {
    return [{ title: "Overview", start: 0, end: content.length, projects: projectsIn(visibleLines, 0, content.length, projectNames) }];
  }

  const splitHeadings = headings.filter((heading) => heading.depth === splitDepth);
  if (splitHeadings.length < 2) {
    return [{ title: "Overview", start: 0, end: content.length, projects: projectsIn(visibleLines, 0, content.length, projectNames) }];
  }

  const sections: MemorySection[] = [];
  const firstHeading = splitHeadings[0];
  if (firstHeading && firstHeading.start > 0) {
    sections.push({
      title: "Overview",
      start: 0,
      end: firstHeading.start,
      projects: projectsIn(visibleLines, 0, firstHeading.start, projectNames),
    });
  }

  splitHeadings.forEach((heading, index) => {
    const nextHeading = splitHeadings[index + 1];
    const end = nextHeading?.start ?? content.length;
    const leadingProjects = projectsIn(visibleLines, 0, headings[0]?.start ?? firstHeading.start, projectNames);
    const inheritedProjects = ancestorProjects(heading, headings, visibleLines, splitDepth, projectNames);
    const ownProjects = projectsIn(visibleLines, heading.start, end, projectNames);
    sections.push({
      title: heading.title,
      start: heading.start,
      end,
      projects: mergeProjects(projectNames, leadingProjects, inheritedProjects, ownProjects),
    });
  });

  return sections;
}

function scanMarkdown(content: string): ScannedMarkdown {
  const headings: Heading[] = [];
  const visibleLines: Line[] = [];
  let fence: { marker: "`" | "~"; length: number } | null = null;

  for (let start = 0; start < content.length;) {
    const newline = content.indexOf("\n", start);
    const end = newline < 0 ? content.length : newline + 1;
    let text = content.slice(start, newline < 0 ? content.length : newline);
    if (text.endsWith("\r")) text = text.slice(0, -1);
    const parseText = start === 0 ? text.replace(/^\uFEFF/, "") : text;

    if (fence) {
      const closing = new RegExp(`^ {0,3}${fence.marker === "`" ? "`" : "~"}{${fence.length},}[ \\t]*$`);
      if (closing.test(parseText)) fence = null;
      start = end;
      continue;
    }

    const fenceMatch = fencePattern.exec(parseText);
    if (fenceMatch?.[1]) {
      const marker = fenceMatch[1][0] === "`" ? "`" : "~";
      const info = fenceMatch[2] ?? "";
      if (marker !== "`" || !info.includes("`")) {
        fence = { marker, length: fenceMatch[1].length };
      }
      start = end;
      continue;
    }

    const line = { start, end, text: parseText };
    visibleLines.push(line);
    const headingMatch = headingPattern.exec(parseText);
    if (headingMatch?.[1]) {
      const rawTitle = headingMatch[2] ?? "";
      const title = rawTitle.replace(/[ \t]+#+[ \t]*$/, "").trim() || "Untitled section";
      headings.push({ depth: headingMatch[1].length, title, start });
    }
    start = end;
  }

  return { headings, visibleLines };
}

function ancestorProjects(target: Heading, headings: Heading[], lines: Line[], splitDepth: number, projectNames: Map<string, string>): string[] {
  const ancestors: Heading[] = [];
  for (const heading of headings) {
    if (heading.start >= target.start) break;
    while (ancestors.at(-1) && (ancestors.at(-1)?.depth ?? 0) >= heading.depth) ancestors.pop();
    if (heading.depth < splitDepth) ancestors.push(heading);
  }

  const projects: string[] = [];
  for (const ancestor of ancestors) {
    const firstChild = headings.find((heading) => heading.start > ancestor.start && heading.depth > ancestor.depth);
    const end = firstChild?.start ?? target.start;
    for (const project of projectsIn(lines, ancestor.start, end, projectNames)) addProject(projects, project, projectNames);
  }
  return projects;
}
function chooseSplitDepth(headings: Heading[]): number | null {
  const counts = new Map<number, number>();
  for (const heading of headings) counts.set(heading.depth, (counts.get(heading.depth) ?? 0) + 1);
  const repeated = [...counts].filter(([, count]) => count > 1).map(([depth]) => depth);
  return repeated.length > 0 ? Math.min(...repeated) : null;
}

function projectsIn(lines: Line[], start: number, end: number, projectNames: Map<string, string>): string[] {
  const projects: string[] = [];
  for (const line of lines) {
    if (line.start < start || line.start >= end) continue;

    const heading = headingPattern.exec(line.text);
    if (heading?.[2]) {
      const title = heading[2].replace(/[ \t]+#+[ \t]*$/, "").trim();
      const path = /^`([^`]+)`$/.exec(title)?.[1]?.trim() ?? title;
      if (isAbsolutePath(path)) addProject(projects, path, projectNames);
    }

    const hasMetadataContext = /^ {0,3}(?:[-*+]\s+)?(?:applies_to|cwd|project(?:_path)?)\s*[:=]/i.test(line.text);
    const hasExplicitAnnotation = /\([^)]*\b(?:cwd|project(?:_path)?)\s*[:=][^)]*\)/i.test(line.text);
    if (!hasMetadataContext && !hasExplicitAnnotation) continue;

    const metadata = /(?:^|[\s:;(,])(?<key>cwd|project(?:_path)?)\s*[:=]\s*(?:"(?<double>[^"]+)"|'(?<single>[^']+)'|(?<plain>[^;,\)\r\n]+))/gi;
    for (const match of line.text.matchAll(metadata)) {
      const key = match.groups?.key?.toLowerCase();
      const value = (match.groups?.double ?? match.groups?.single ?? match.groups?.plain ?? "").trim();
      if (!value) continue;
      if ((key === "cwd" || key === "project_path") && !isAbsolutePath(value)) continue;
      addProject(projects, value, projectNames);
    }
  }
  return projects;
}

function isAbsolutePath(value: string): boolean {
  return /^(?:[A-Za-z]:[\\/]|(?:\\\\|\/\/)[^\\/]+[\\/][^\\/]+|\/[^/])/.test(value.trim());
}

function mergeProjects(projectNames: Map<string, string>, ...collections: string[][]): string[] {
  const merged: string[] = [];
  for (const collection of collections) for (const project of collection) addProject(merged, project, projectNames);
  return merged;
}

function addProject(projects: string[], value: string, projectNames: Map<string, string>): void {
  const normalized = normalizeProject(value);
  if (!normalized) return;
  const display = projectNames.get(normalized) ?? value;
  projectNames.set(normalized, display);
  if (!projects.some((project) => normalizeProject(project) === normalized)) projects.push(display);
}

function normalizeProject(value: string): string {
  let path = value.trim();
  if (path.startsWith("\\\\?\\UNC\\")) path = `\\\\${path.slice(8)}`;
  else if (path.startsWith("\\\\?\\")) path = path.slice(4);

  const isWindowsPath = /^[A-Za-z]:[\\/]/.test(path) || path.startsWith("\\\\") || path.startsWith("//");
  if (isWindowsPath) {
    const slashes = path.replace(/\\/g, "/");
    const normalized = slashes.startsWith("//")
      ? `//${slashes.slice(2).replace(/\/{2,}/g, "/")}`
      : slashes.replace(/\/{2,}/g, "/");
    return normalized.replace(/\/+$/, "").toLowerCase();
  }

  if (path.startsWith("/")) return path.replace(/\/{2,}/g, "/").replace(/\/+$/, "");
  return path;
}
