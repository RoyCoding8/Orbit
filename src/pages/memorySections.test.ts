import { describe, expect, it } from "vitest";
import { parseMemorySections } from "./memorySections";

function expectPartition(source: string, sections: ReturnType<typeof parseMemorySections>) {
  expect(sections.map(({ start, end }) => source.slice(start, end)).join("")).toBe(source);
  expect(sections[0]?.start ?? 0).toBe(0);
  for (let index = 1; index < sections.length; index += 1) {
    expect(sections[index]?.start).toBe(sections[index - 1]?.end);
  }
  expect(sections.at(-1)?.end ?? 0).toBe(source.length);
}

describe("parseMemorySections", () => {
  it("splits at the shallowest repeated heading and keeps nested content with its section", () => {
    const source = "# Task Group: Agent Society\n\nscope: prose\n\napplies_to: cwd=D:\\AI\\Agent-Society-v2; reuse_rule=when relevant\n\n## Task 1: Discovery\n\n### Evidence\nFirst.\n\n## Task 2: Transfer\nSecond.\n";
    const sections = parseMemorySections(source);

    expect(sections.map(({ title }) => title)).toEqual(["Overview", "Task 1: Discovery", "Task 2: Transfer"]);
    expect(sections.map(({ projects }) => projects)).toEqual([
      ["D:\\AI\\Agent-Society-v2"],
      ["D:\\AI\\Agent-Society-v2"],
      ["D:\\AI\\Agent-Society-v2"],
    ]);
    expect(source.slice(sections[1]?.start, sections[1]?.end)).toContain("### Evidence");
    expectPartition(source, sections);
  });

  it("splits project sections below a single document title", () => {
    const source = "# Memory index\n\nShared preamble.\n\n## D:\\AI\\Orbit\nOrbit notes.\n\n## /work/other\nOther notes.\n";
    const sections = parseMemorySections(source);

    expect(sections.map(({ title }) => title)).toEqual(["Overview", "D:\\AI\\Orbit", "/work/other"]);
    expect(sections[1]?.projects).toEqual(["D:\\AI\\Orbit"]);
    expect(sections[2]?.projects).toEqual(["/work/other"]);
    expectPartition(source, sections);
  });

  it("splits multiple top-level task groups and ignores headings in closed or unclosed fences", () => {
    const source = "# Task Group: First\n\napplies_to: cwd=D:\\AI\\Orbit\n\n```md\n# Fake task group\n```\n\n## Task 1\nFirst task.\n\n# Task Group: Second\n\n## Task 2\nSecond task.\n\n~~~md\n# Fake final group\n";
    const sections = parseMemorySections(source);

    expect(sections.map(({ title }) => title)).toEqual(["Task Group: First", "Task Group: Second"]);
    expect(sections.map(({ projects }) => projects)).toEqual([["D:\\AI\\Orbit"], []]);
    expectPartition(source, sections);

    const unclosedBacktick = "# Notes\n\n```md\n## Fake section\n## Still fenced\n";
    expect(parseMemorySections(unclosedBacktick).map(({ title }) => title)).toEqual(["Overview"]);
  });

  it("ignores heading and metadata lookalikes inside fenced blocks and preserves CRLF, BOM, and Unicode offsets", () => {
    const source = "\uFEFF# Notes\r\n\r\n```md\r\n## Fake section\r\napplies_to: cwd=D:\\private\r\n```\r\n\r\n## One 🌱\r\nText.\r\n\r\n## Two 🧭\r\nDone.\r\n";
    const sections = parseMemorySections(source);

    expect(sections.map(({ title }) => title)).toEqual(["Overview", "One 🌱", "Two 🧭"]);
    expect(sections.every(({ projects }) => projects.length === 0)).toBe(true);
    expectPartition(source, sections);
  });
  it("collects explicit local and rollout source hints, but ignores prose mentions", () => {
    const source = "# Evidence\n\nI often work in D:\\AI\\Orbit.\n\n- rollout (cwd=C:\\Users\\roy\\Documents\\Codex; rollout_path=notes.md)\n\n## First\nNotes.\n\n## Second\nThis says project=/fake in prose.\n";
    const sections = parseMemorySections(source);

    expect(sections[0]?.projects).toEqual(["C:\\Users\\roy\\Documents\\Codex"]);
    expect(sections[1]?.projects).toEqual(["C:\\Users\\roy\\Documents\\Codex"]);
    expect(sections[2]?.projects).toEqual(["C:\\Users\\roy\\Documents\\Codex"]);
    expectPartition(source, sections);
  });

  it("uses platform-aware path identity and keeps the first display spelling across sections", () => {
    const source = "# Paths\n\n## Windows one\ncwd=D:\\Work\\A\n\n## Windows two\ncwd=d:/work/a/\n\n## Unix upper\nproject: /work/A\n\n## Unix lower\nproject: /work/a\n\n## Device drive\ncwd=\\\\?\\C:\\Repo\n\n## Plain drive\ncwd=c:/repo\n\n## Device UNC\ncwd=\\\\?\\UNC\\Server\\Share\\src\n\n## Plain UNC\ncwd=//server/share/src\n\n## Backtick path\n\n## `D:\\Other`\n\n## Plain path\ncwd=d:/other\n";
    const sections = parseMemorySections(source);

    expect(sections.map(({ projects }) => projects)).toEqual([
      [],
      ["D:\\Work\\A"],
      ["D:\\Work\\A"],
      ["/work/A"],
      ["/work/a"],
      ["\\\\?\\C:\\Repo"],
      ["\\\\?\\C:\\Repo"],
      ["\\\\?\\UNC\\Server\\Share\\src"],
      ["\\\\?\\UNC\\Server\\Share\\src"],
      [],
      ["D:\\Other"],
      ["D:\\Other"],
    ]);
    expectPartition(source, sections);
  });

  it("keeps multiple explicit projects together and leaves unknown scope empty", () => {
    const source = "# Notes\n\nproject: Alpha\nproject: Beta\nscope: prose\n\n## First\nOne.\n\n## Second\nTwo.\n";
    const sections = parseMemorySections(source);

    expect(sections[0]?.projects).toEqual(["Alpha", "Beta"]);
    expect(sections[1]?.projects).toEqual(["Alpha", "Beta"]);
    expect(sections[2]?.projects).toEqual(["Alpha", "Beta"]);
    expect(parseMemorySections("scope: prose\n\nNo headings here.")[0]?.projects).toEqual([]);
    expectPartition(source, sections);
  });

  it("returns one overview for unstructured or non-repeated headings", () => {
    for (const source of ["Plain text.\r\n", "# One title\n\n## One child\nBody.\n"]) {
      const sections = parseMemorySections(source);
      expect(sections).toHaveLength(1);
      expect(sections[0]?.title).toBe("Overview");
      expectPartition(source, sections);
    }
    expect(parseMemorySections("")).toEqual([]);
  });
});
