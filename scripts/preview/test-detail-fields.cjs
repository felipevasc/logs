const assert = require("node:assert/strict");
const test = require("node:test");

globalThis.jsyaml = require("../../frontend/vendor/js-yaml/js-yaml.min.js");
const { buildTree, detect, format } = require("../../frontend/detail-fields.js");

test("campos com pontos criam todos os níveis sem perder o valor do pai", () => {
  const tree = buildTree([
    { key: "message", value: '{"agent":{"id":"abc"}}', filterValue: "original" },
    { key: "fields.network.bytes", value: 192, filterValue: 192 },
    { key: "fields.network.community_id", value: "wm3fFUcpDi8zdsOH+/+nSy3I8Is=" },
  ]);
  const message = tree.find((node) => node.path === "message");
  assert.equal(message.value, '{"agent":{"id":"abc"}}');
  assert.equal(message.children[0].children[0].path, "message.agent.id");
  assert.equal(tree.find((node) => node.path === "fields").children[0].children[0].path, "fields.network.bytes");
  assert.equal(tree.find((node) => node.path === "fields").children[0].children[1].children.length, 0);
  const nestedQuery = buildTree([{ key: "query", value: "a.b=value" }]);
  assert.equal(nestedQuery[0].children[0].children[0].path, "query.a.b");
});

test("formatos estruturados geram filhos e mantém a formatação", () => {
  const query = detect("one=hello+world&two=42&two=43");
  assert.equal(query.type, "query");
  assert.deepEqual(query.data.two, ["42", "43"]);
  assert.match(format(null, query).html, /j-key/);

  const yaml = detect("service:\n  name: api\n  retries: 3");
  assert.equal(yaml.type, "yaml");
  assert.equal(yaml.data.service.retries, 3);

  const stack = detect("Error: falha\n    at service.run (app.js:10)");
  assert.equal(stack.type, "stack");
  assert.equal(stack.data.frames.length, 2);
  assert.match(format(null, stack).html, /detail-stack-frame/);
});

test("conteúdo não confiável permanece escapado na visualização", () => {
  const value = '{"key":"<img src=x onerror=alert(1)>"}';
  const result = format(value);
  assert.ok(!result.html.includes("<img"));
  assert.match(result.html, /&lt;img/);
});
