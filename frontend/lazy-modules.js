/* Scripts of rarely used pages are loaded the first time the page opens, so a basic PC
   parses less code at startup. Order inside a group is preserved. */
window.LazyModules = (() => {
  "use strict";
  const groups = {
    extraction: ["extraction-model.js", "columns-engine.js", "columns.js", "extraction.js"],
    "log-split": ["log-split.js"],
  };
  const loading = new Map();
  const present = src => [...document.scripts].some(s => s.getAttribute("src") === src);
  function script(src) {
    if (present(src)) return Promise.resolve();
    return new Promise((resolve, reject) => {
      const element = document.createElement("script");
      element.src = src; element.async = false;
      element.onload = () => resolve();
      element.onerror = () => { element.remove(); reject(new Error(`Não foi possível carregar ${src}.`)); };
      document.body.append(element);
    });
  }
  function ensure(name) {
    const files = groups[name];
    if (!files) return Promise.resolve();
    if (!loading.has(name)) {
      const chain = files.reduce((previous, src) => previous.then(() => script(src)), Promise.resolve())
        .catch(error => { loading.delete(name); throw error; });
      loading.set(name, chain);
    }
    return loading.get(name);
  }
  return { ensure, groups: () => structuredClone(groups) };
})();
