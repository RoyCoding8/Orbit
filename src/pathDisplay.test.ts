import { expect, it } from "vitest";
import { displayPath } from "./pathDisplay";

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
