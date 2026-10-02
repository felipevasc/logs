import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { readFileSync } from "node:fs";

const read = path => readFileSync(new URL(`../../${path}`, import.meta.url), "utf8");
const workbench = read("frontend/analysis-workbench.css"), base = read("frontend/styles.css");
const workspace = read("frontend/workspace.css"), contexts = read("frontend/workspace-context.css");
const browser = read("scripts/preview/test-analysis-workbench.mjs");
const probe = browser.slice(browser.indexOf("function measurePivotChipContrast()"), browser.indexOf("function assertPivotChipContrast("));
const plain = value => JSON.parse(JSON.stringify(value));
const rule = (css, selector) => css.slice(css.indexOf(`${selector} {`) + selector.length + 2).split("}")[0];
const token = (css, selector, name) => {
  assert.ok(css.includes(`${selector} {`), `missing palette ${selector}`);
  return rule(css, selector).match(new RegExp(`${name}:\\s*([^;]+)`))[1].trim();
};
const computed = hex => {
  if (hex.length === 4) hex = `#${[...hex.slice(1)].map(char => char + char).join("")}`;
  const values = hex.slice(1).match(/../g).map(pair => Number.parseInt(pair, 16));
  return `rgba(${values.slice(0, 3).join(", ")}, ${values.length === 4 ? values[3] / 255 : 1})`;
};

function fixture({ color = "rgb(0, 0, 0)", surface = "rgba(0, 0, 0, 0)", ancestor = "rgb(255, 255, 255)", visible = true } = {}) {
  const node = (tagName, className, parentElement, backgroundColor) => ({
    tagName, id: "", classList: className ? [className] : [], parentElement,
    style: { color, backgroundColor, backgroundImage: "none", opacity: "1", filter: "none", mixBlendMode: "normal" },
  });
  const root = node("HTML", "", null, "rgb(255, 255, 255)");
  const zone = node("DIV", "cube-zone", root, ancestor);
  const items = node("DIV", "cube-zone-items", zone, "rgba(0, 0, 0, 0)");
  const chip = node("SPAN", "cube-chip", items, surface);
  const label = node("SPAN", "", chip, "rgba(0, 0, 0, 0)"); label.textContent = "Nível";
  chip.getClientRects = () => visible ? [{}] : []; chip.querySelector = () => label;
  const context = vm.createContext({ document: { querySelectorAll: () => [chip] }, getComputedStyle: node => node.style });
  vm.runInContext(probe, context);
  return { measure: () => plain(context.measurePivotChipContrast()), chip, zone, root };
}

test("light pivot chips only override color with the existing workspace accent-strong token", () => {
  assert.equal(rule(workbench, 'html[data-theme="light"] .cube-chip').trim(), "color:var(--accent-strong);");
  assert.equal([...workbench.matchAll(/\.cube-chip\s*\{/g)].length, 1, "no dark, geometry or interaction overrides");
  assert.match(base, /\.cube-chip\s*\{\s*border-radius: 6px; color: #9fc0ff;\s*\}/, "the existing dark label color stays intact");
  assert.match(base, /html\[data-theme="light"\] \.chip \{ color: var\(--accent-strong\); \}/, "use the same functional token as light filter chips");
});

test("current Análise and Caso palettes clear the chip-label threshold in both themes", () => {
  for (const theme of ["dark", "light"]) for (const scope of ["dataset", "case"]) {
    const selector = `html[data-theme="${theme}"][data-workspace="${scope}"]`;
    const color = theme === "light" ? token(contexts, selector, "--accent-strong") : "#9fc0ff";
    const surface = token(contexts, selector, "--accent-soft");
    const ancestor = token(workspace, `html[data-theme="${theme}"]`, "--bg-2");
    const [chip] = fixture({ color: computed(color), surface: computed(surface), ancestor: computed(ancestor) }).measure();
    assert.ok(chip.ratio >= 4.5, `${scope}/${theme}: ${chip.ratio.toFixed(2)}:1`);
    assert.equal(chip.opaqueAncestor, "div.cube-zone");
    if (theme === "light") {
      const [old] = fixture({ color: computed("#9fc0ff"), surface: computed(surface), ancestor: computed(ancestor) }).measure();
      assert.ok(old.ratio < 4.5, `${scope}: the previous pale label must reproduce the regression`);
    }
  }
});

test("browser probe composites chip alpha over the real opaque zone rather than the white root", () => {
  const [chip] = fixture({ surface: "rgba(200, 100, 50, 0.25)", ancestor: "rgb(20, 40, 60)" }).measure();
  assert.deepEqual(chip.background, [65, 55, 57.5, 1]);
  assert.equal(chip.opaqueAncestor, "div.cube-zone");
  assert.equal(chip.layers.length, 4, "transparent label and item wrappers remain in the evidence");
  assert.equal(fixture().measure()[0].ratio, 21, "black on white validates the luminance calculation");
  assert.deepEqual(fixture({ visible: false }).measure(), [], "folded chips are not reported as visible measurements");
});

test("browser probe fails closed when a reliable effective surface cannot be measured", () => {
  const missing = fixture({ ancestor: "rgba(0, 0, 0, 0)" }); missing.root.style.backgroundColor = "rgba(0, 0, 0, 0)";
  assert.throws(missing.measure, /no opaque ancestor/);
  const image = fixture(); image.chip.style.backgroundImage = "linear-gradient(black, white)";
  assert.throws(image.measure, /Unsupported contrast layer/);
  const opacity = fixture(); opacity.root.style.opacity = "0.5";
  assert.throws(opacity.measure, /Unsupported contrast layer/, "group opacity above the opaque ancestor still affects effective contrast");
});
