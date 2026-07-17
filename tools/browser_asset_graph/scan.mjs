import fs from "node:fs";
import process from "node:process";

import { parse } from "acorn";

if (Number.parseInt(process.versions.node.split(".", 1)[0], 10) < 18) {
  throw new Error(`browser asset graph requires Node >=18, found ${process.versions.node}`);
}

const SCOPE_NODES = new Set([
  "BlockStatement",
  "CatchClause",
  "ClassBody",
  "ForInStatement",
  "ForOfStatement",
  "ForStatement",
  "StaticBlock",
  "SwitchStatement",
]);

class Scope {
  constructor(parent, kind) {
    this.parent = parent;
    this.kind = kind;
    this.bindings = new Set();
  }

  functionOwner() {
    let scope = this;
    while (scope.parent && scope.kind !== "function" && scope.kind !== "program") {
      scope = scope.parent;
    }
    return scope;
  }

  resolves(name) {
    for (let scope = this; scope; scope = scope.parent) {
      if (scope.bindings.has(name)) return true;
    }
    return false;
  }
}

const childNodes = function* (node) {
  for (const [key, value] of Object.entries(node)) {
    if (key === "loc" || key === "start" || key === "end") continue;
    if (Array.isArray(value)) {
      for (const item of value) {
        if (item && typeof item.type === "string") yield item;
      }
    } else if (value && typeof value.type === "string") {
      yield value;
    }
  }
};

const bindPattern = (pattern, scope) => {
  if (!pattern) return;
  switch (pattern.type) {
    case "Identifier":
      scope.bindings.add(pattern.name);
      return;
    case "RestElement":
      bindPattern(pattern.argument, scope);
      return;
    case "AssignmentPattern":
      bindPattern(pattern.left, scope);
      return;
    case "ArrayPattern":
      pattern.elements.forEach((item) => bindPattern(item, scope));
      return;
    case "ObjectPattern":
      pattern.properties.forEach((property) =>
        bindPattern(property.type === "RestElement" ? property.argument : property.value, scope),
      );
      return;
    default:
      return;
  }
};

const buildScopes = (ast) => {
  const root = new Scope(null, "program");
  const scopes = new WeakMap([[ast, root]]);

  const declare = (node, current) => {
    if (!node || typeof node.type !== "string") return;
    let scope = current;
    if (node !== ast && SCOPE_NODES.has(node.type)) {
      scope = new Scope(current, "block");
      scopes.set(node, scope);
      if (node.type === "CatchClause") bindPattern(node.param, scope);
    }

    if (node.type === "ImportDeclaration") {
      node.specifiers.forEach((specifier) => bindPattern(specifier.local, scope));
    } else if (node.type === "VariableDeclaration") {
      const owner = node.kind === "var" ? scope.functionOwner() : scope;
      node.declarations.forEach((declaration) => bindPattern(declaration.id, owner));
    } else if (node.type === "FunctionDeclaration") {
      if (node.id) scope.bindings.add(node.id.name);
      const functionScope = new Scope(scope, "function");
      scopes.set(node, functionScope);
      node.params.forEach((parameter) => bindPattern(parameter, functionScope));
      for (const parameter of node.params) declare(parameter, functionScope);
      declare(node.body, functionScope);
      return;
    } else if (node.type === "FunctionExpression" || node.type === "ArrowFunctionExpression") {
      const functionScope = new Scope(scope, "function");
      scopes.set(node, functionScope);
      if (node.type === "FunctionExpression" && node.id) {
        functionScope.bindings.add(node.id.name);
      }
      node.params.forEach((parameter) => bindPattern(parameter, functionScope));
      for (const parameter of node.params) declare(parameter, functionScope);
      declare(node.body, functionScope);
      return;
    } else if (node.type === "ClassDeclaration") {
      if (node.id) scope.bindings.add(node.id.name);
      const classScope = new Scope(scope, "block");
      scopes.set(node, classScope);
      if (node.id) classScope.bindings.add(node.id.name);
      if (node.superClass) declare(node.superClass, scope);
      declare(node.body, classScope);
      return;
    } else if (node.type === "ClassExpression") {
      const classScope = new Scope(scope, "block");
      scopes.set(node, classScope);
      if (node.id) classScope.bindings.add(node.id.name);
      if (node.superClass) declare(node.superClass, scope);
      declare(node.body, classScope);
      return;
    }

    for (const child of childNodes(node)) declare(child, scope);
  };

  declare(ast, root);
  return { root, scopes };
};

const staticString = (node) => {
  if (node?.type === "Literal" && typeof node.value === "string") return node.value;
  if (node?.type === "TemplateLiteral" && node.expressions.length === 0) {
    return node.quasis[0]?.value?.cooked ?? node.quasis[0]?.value?.raw ?? null;
  }
  return null;
};

const isImportMetaUrl = (node) =>
  node?.type === "MemberExpression" &&
  !node.computed &&
  node.property?.type === "Identifier" &&
  node.property.name === "url" &&
  node.object?.type === "MetaProperty" &&
  node.object.meta?.name === "import" &&
  node.object.property?.name === "meta";

const staticUrl = (node, scope) => {
  if (
    node?.type !== "NewExpression" ||
    node.callee?.type !== "Identifier" ||
    node.callee.name !== "URL" ||
    scope.resolves("URL") ||
    node.arguments.length < 2 ||
    !isImportMetaUrl(node.arguments[1])
  ) {
    return null;
  }
  return staticString(node.arguments[0]);
};

const scanSource = ({ id, path, role, source, source_type: sourceType }) => {
  if (![id, path, role, source, sourceType].every((value) => typeof value === "string")) {
    throw new Error("browser asset scanner received a malformed source row");
  }
  let ast;
  try {
    ast = parse(source, {
      ecmaVersion: "latest",
      locations: true,
      sourceType,
    });
  } catch (error) {
    throw new Error(`${path}: ${error.message}`);
  }
  const { root, scopes } = buildScopes(ast);
  const references = [];
  const failures = [];

  const record = (request, node, kind) => {
    if (typeof request !== "string") {
      failures.push(
        `${kind} at ${path}:${node.loc.start.line}:${node.loc.start.column + 1} is not statically resolvable`,
      );
      return;
    }
    references.push({
      column: node.loc.start.column + 1,
      kind,
      line: node.loc.start.line,
      request,
    });
  };

  const visit = (node, current) => {
    if (!node || typeof node.type !== "string") return;
    const scope = scopes.get(node) ?? current;
    if (
      node.type === "ImportDeclaration" ||
      node.type === "ExportAllDeclaration" ||
      (node.type === "ExportNamedDeclaration" && node.source)
    ) {
      record(staticString(node.source), node, "module");
    } else if (node.type === "ImportExpression") {
      record(staticString(node.source) ?? staticUrl(node.source, scope), node, "dynamic-import");
    } else if (
      node.type === "CallExpression" &&
      node.callee?.type === "Identifier" &&
      node.callee.name === "require" &&
      !scope.resolves("require")
    ) {
      record(node.arguments.length === 1 ? staticString(node.arguments[0]) : null, node, "require");
    } else if (
      node.type === "CallExpression" &&
      node.callee?.type === "Identifier" &&
      node.callee.name === "importScripts" &&
      !scope.resolves("importScripts")
    ) {
      if (node.arguments.length === 0) record(null, node, "import-scripts");
      node.arguments.forEach((argument) => record(staticString(argument), node, "import-scripts"));
    } else if (
      node.type === "CallExpression" &&
      node.callee?.type === "Identifier" &&
      node.callee.name === "fetch" &&
      !scope.resolves("fetch") &&
      node.arguments.length > 0
    ) {
      const request = staticString(node.arguments[0]) ?? staticUrl(node.arguments[0], scope);
      if (request !== null) record(request, node, "fetch");
    } else if (
      node.type === "NewExpression" &&
      node.callee?.type === "Identifier" &&
      (node.callee.name === "Worker" || node.callee.name === "SharedWorker") &&
      !scope.resolves(node.callee.name)
    ) {
      const request = node.arguments.length
        ? staticString(node.arguments[0]) ?? staticUrl(node.arguments[0], scope)
        : null;
      record(request, node, node.callee.name === "Worker" ? "worker" : "shared-worker");
    }
    for (const child of childNodes(node)) visit(child, scope);
  };

  visit(ast, root);
  if (failures.length) throw new Error(failures.join("\n"));
  references.sort((left, right) =>
    left.request.localeCompare(right.request) ||
    left.kind.localeCompare(right.kind) ||
    left.line - right.line ||
    left.column - right.column,
  );
  return { id, path, references, role };
};

const started = process.hrtime.bigint();
const raw = fs.readFileSync(0, "utf8");
const payload = JSON.parse(raw);
if (!Array.isArray(payload.sources)) {
  throw new Error("browser asset scanner payload has no sources array");
}
const results = payload.sources.map(scanSource);
const elapsedNs = Number(process.hrtime.bigint() - started);
process.stdout.write(
  JSON.stringify({
    results,
    telemetry: {
      elapsed_ns: elapsedNs,
      rss_bytes: process.memoryUsage().rss,
      source_bytes: Buffer.byteLength(raw),
      source_count: payload.sources.length,
    },
  }),
);
