// Diff2Html UI with a small highlight.js core: only the languages wshm repos
// commonly touch, to keep the bundle (inlined in static pages) small.
import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import css from "highlight.js/lib/languages/css";
import go from "highlight.js/lib/languages/go";
import ini from "highlight.js/lib/languages/ini";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import markdown from "highlight.js/lib/languages/markdown";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";
import { Diff2HtmlUI as Base } from "diff2html/lib-esm/ui/js/diff2html-ui-base.js";

for (const [name, lang] of Object.entries({ bash, css, go, ini, java, javascript, json, markdown, python, rust, typescript, xml, yaml })) {
  hljs.registerLanguage(name, lang);
}
hljs.registerAliases(["toml"], { languageName: "ini" });
hljs.registerAliases(["svelte", "html", "vue"], { languageName: "xml" });

export class Diff2HtmlUI extends Base {
  constructor(target, diffInput, config = {}) {
    super(target, diffInput, config, hljs);
  }
}
