/// <reference types="vite/client" />

declare module "@m3e/web/theme";
declare module "@m3e/web/button";

declare namespace React.JSX {
  interface IntrinsicElements {
    "m3e-theme": React.DetailedHTMLProps<React.HTMLAttributes<HTMLElement>, HTMLElement> & {
      color?: string;
      scheme?: "auto" | "light" | "dark";
      density?: number;
      "strong-focus"?: boolean;
    };
  }
}