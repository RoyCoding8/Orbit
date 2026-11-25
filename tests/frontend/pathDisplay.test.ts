import { expect, it } from "vitest";
import { displayPath, normalizePathIdentity } from "../../src/lib/pathDisplay";

it.each([
  ["\\\\?\\D:\\AI\\Orbit", "D:\\AI\\Orbit"],
  ["Selected project · \\\\?\\D:\\AI\\Orbit", "Selected project · D:\\AI\\Orbit"],
  ["\\\\?\\UNC\\Server\\Share\\src", "\\\\Server\\Share\\src"],
  ["\\\\?\\unc\\Server\\Share\\src", "\\\\Server\\Share\\src"],
  ["D:\\AI\\Agent-Society-v2", "D:\\AI\\Agent-Society-v2"],
  ["/work/project-name", "/work/project-name"],
  ["D--AI-Agent-Society-v2", "D--AI-Agent-Society-v2"],
  ["\\\\?\\Volume{example}\\", "\\\\?\\Volume{example}\\"],
])("displays %s without changing path meaning", (source, expected) => {
  expect(displayPath(source)).toBe(expected);
});

it.each([
  ["\\\\?\\D:\\Work\\Alpha\\", "d:/work/alpha"],
  ["d:/work//alpha/", "d:/work/alpha"],
  ["\\\\?\\UNC\\Server\\Share\\src\\", "//server/share/src"],
  ["/work//Beta/", "/work/Beta"],
  ["/", "/"],
  ["Unknown project", "Unknown project"],
])("normalizes path identity for %s", (source, expected) => {
  expect(normalizePathIdentity(source)).toBe(expected);
});
