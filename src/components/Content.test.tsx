import { useState } from "react";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { MarkdownEditor } from "./MarkdownEditor";
import { JsonEditor } from "./JsonEditor";
import { JsonValue } from "./JsonValue";

describe("Markdown editor", () => {
  it("previews the current draft with tables and checklists and keeps it when returning to edit", async () => {
    function Editor() {
      const [value, setValue] = useState("# Original");
      return <MarkdownEditor id="draft" label="Document" value={value} onChange={setValue} />;
    }
    render(<Editor />);
    const draft = "# Revised\n\n| Name | Value |\n| --- | --- |\n| Agent | Orbit |\n\n- [x] Ready\n\n```json\n{\"enabled\":true}\n```";
    fireEvent.change(screen.getByLabelText("Document"), { target: { value: draft } });
    fireEvent.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByRole("heading", { name: "Revised" })).toBeInTheDocument();
    expect(screen.getByRole("table")).toHaveTextContent("AgentOrbit");
    expect(screen.getByRole("checkbox")).toBeChecked();
    expect(screen.getByText('{"enabled":true}')).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(screen.getByLabelText("Document")).toHaveValue(draft);
  });

  it("renders read-only Markdown while keeping metadata, HTML, images and unsafe links inert", async () => {
    const value = '---\nname: example\n---\n# Guide\n\n<script>alert(1)</script>\n\n[Bad](javascript:alert(1)) [Docs](https://example.test/docs) ![Remote](https://example.test/tracker.png)';
    const { container } = render(<MarkdownEditor id="original" label="Original" value={value} readOnly />);
    expect(await screen.findByRole("heading", { name: "Guide" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Docs" })).toHaveAttribute("href", "https://example.test/docs");
    expect(screen.getByText("Bad").closest("a")).toBeNull();
    expect(container.querySelector("script, img")).toBeNull();
    fireEvent.click(screen.getByText("File metadata"));
    expect(screen.getByText("name: example")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Source" }));
    expect(screen.getByLabelText("Original")).toHaveValue(value);
    expect(screen.getByLabelText("Original")).toHaveAttribute("readonly");
  });
});

describe("Structured JSON", () => {
  it("expands nested objects and arrays without losing primitive types or overwhelming the list", () => {
    render(<JsonValue value={{ provider: { enabled: false, timeout: 0, token: null }, models: Array.from({ length: 52 }, (_, index) => `model-${index}`) }} />);
    expect(screen.getByRole("button", { name: /Object 2 fields/ })).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(screen.getByRole("button", { name: /Object 2 fields/ }));
    fireEvent.click(screen.getByRole("button", { name: /provider 3 fields/ }));
    expect(screen.getByText("false")).toBeInTheDocument();
    expect(screen.getByText("0")).toBeInTheDocument();
    expect(screen.getByText("null")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /models 52 items/ }));
    expect(screen.getByText('"model-49"')).toBeInTheDocument();
    expect(screen.queryByText('"model-50"')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show next 2 of 2 remaining" }));
    expect(screen.getByText('"model-51"')).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /provider 3 fields/ }));
    expect(screen.queryByText("false")).not.toBeInTheDocument();
    expect(screen.getByText('"model-51"')).toBeInTheDocument();
  });

  it("keeps incomplete source intact and explores valid edits without rewriting them", () => {
    function Editor() {
      const [value, setValue] = useState('{"nested":');
      return <JsonEditor id="json" label="JSON value" value={value} onChange={setValue} />;
    }
    render(<Editor />);
    const controls = screen.getByRole("group", { name: "JSON display" });
    fireEvent.click(within(controls).getByRole("button", { name: "Explore" }));
    expect(screen.getByRole("status")).toHaveTextContent("Enter valid JSON");
    fireEvent.click(within(controls).getByRole("button", { name: "JSON" }));
    expect(screen.getByLabelText("JSON value")).toHaveValue('{"nested":');
    const draft = '{ "nested": { "value": "changed" } }';
    fireEvent.change(screen.getByLabelText("JSON value"), { target: { value: draft } });
    fireEvent.click(within(controls).getByRole("button", { name: "Explore" }));
    fireEvent.click(screen.getByRole("button", { name: /Object 1 fields/ }));
    fireEvent.click(screen.getByRole("button", { name: /nested 1 fields/ }));
    expect(screen.getByText('"changed"')).toBeInTheDocument();
    fireEvent.click(within(controls).getByRole("button", { name: "JSON" }));
    expect(screen.getByLabelText("JSON value")).toHaveValue(draft);
  });
});
