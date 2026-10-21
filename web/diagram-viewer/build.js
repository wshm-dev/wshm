// Bundles the viewer into ../../src/diagrams/assets (embedded in wshm).
// Run: bun install && bun run build
const out = "../../src/diagrams/assets";
const result = await Bun.build({
  entrypoints: ["src/app.js"],
  target: "browser",
  format: "iife",
  minify: true,
  define: {
    "process.env.NODE_ENV": '"production"',
    // zustand (React Flow) reads import.meta.env, invalid in a classic script.
    "import.meta.env": '{"MODE":"production"}',
  },
});
if (!result.success) {
  for (const log of result.logs) console.error(log);
  process.exit(1);
}
const js = await result.outputs[0].text();
if (js.includes("import.meta")) throw new Error("import.meta left in the bundle");
await Bun.write(`${out}/viewer.js`, js);
const css = await Promise.all(
  [
    "node_modules/@xyflow/react/dist/style.css",
    "node_modules/diff2html/bundles/css/diff2html.min.css",
    "node_modules/highlight.js/styles/github.min.css",
    "src/viewer.css",
  ].map((p) => Bun.file(p).text()),
);
await Bun.write(`${out}/viewer.css`, css.join("\n"));
console.log(`viewer.js ${js.length} bytes, viewer.css ${css.join("\n").length} bytes`);
