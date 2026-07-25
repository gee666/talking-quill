import { readFile, readdir } from 'node:fs/promises';
import { relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const ROOT = resolve(fileURLToPath(new URL('..', import.meta.url)));
const SOURCE_ROOT = resolve(ROOT, 'app/src');

export const APPROVED_NETWORK_BOUNDARIES = Object.freeze({
  'app/src/main/providers/json-transport.ts': Object.freeze({
    category: 'provider-or-update',
    reason: 'Pinned DNS/SSRF-validated JSON transport; caller assigns provider or update category.',
    tokens: Object.freeze(['node:http', 'node:https', 'node:net']),
  }),
  'app/src/main/transcription/model-manager.ts': Object.freeze({
    category: 'model-download',
    reason: 'Manifest-pinned, checksum-verified Hugging Face model downloader.',
    tokens: Object.freeze(['fetch-call']),
  }),
  'app/src/main/security/protocol.ts': Object.freeze({
    category: 'local-file-only',
    reason: 'Electron net.fetch is restricted to a pathToFileURL generated under rendererRoot.',
    tokens: Object.freeze(['electron-net-fetch']),
  }),
  'app/src/workers/whisper/network-guard.ts': Object.freeze({
    category: 'deny-only',
    reason: 'Installs and self-tests denial stubs before loading the offline worker payload.',
    tokens: Object.freeze([
      'node:http',
      'node:https',
      'node:http2',
      'node:net',
      'node:tls',
      'node:dns',
      'node:dns/promises',
      'node:child_process',
      'fetch-call',
    ]),
  }),
  'app/src/main/security/provider-endpoint-policy.ts': Object.freeze({
    category: 'address-validation-only',
    reason: 'Uses DNS lookup plus node:net isIP for pinned endpoint validation.',
    tokens: Object.freeze(['node:dns/promises', 'node:net']),
  }),
  'app/src/main/providers/pi-discovery.ts': Object.freeze({
    category: 'local-command-locator-only',
    reason:
      'Executes only canonical SystemRoot/System32/where.exe with fixed pi.cmd arguments, no shell, bounded output, and candidate revalidation.',
    tokens: Object.freeze(['node:child_process']),
  }),
  'app/src/main/providers/pi.ts': Object.freeze({
    category: 'provider-cli-process-only',
    reason:
      'Launches only the package-identity-validated Pi JavaScript entry with fixed hardened arguments and bounded stdio.',
    tokens: Object.freeze(['node:child_process']),
  }),
  'app/src/main/helper/helper-client.ts': Object.freeze({
    category: 'native-helper-process-only',
    reason:
      'Spawns only the resolved bundled talking-quill-helper executable with fixed stdio IPC.',
    tokens: Object.freeze(['node:child_process']),
  }),
  'app/src/main/data/native-owned-tree-removal.ts': Object.freeze({
    category: 'native-owned-data-removal-only',
    reason: 'Executes only the bundled helper identity-bound reset mode with fixed arguments.',
    tokens: Object.freeze(['node:child_process']),
  }),
  'app/src/main/security/redaction.ts': Object.freeze({
    category: 'address-validation-only',
    reason: 'Uses node:net isIP only while redacting endpoint values.',
    tokens: Object.freeze(['node:net']),
  }),
});

const NETWORK_MODULES = new Set([
  'http',
  'https',
  'http2',
  'net',
  'tls',
  'dns',
  'dns/promises',
  'child_process',
  'undici',
]);
const SOCKET_METHODS = new Set(['connect', 'createConnection', 'request', 'get']);
const BROWSER_NETWORK_CONSTRUCTORS = new Set(['WebSocket', 'EventSource', 'XMLHttpRequest']);

export function detectNetworkTokens(source, fileName = 'network-boundary.ts') {
  const tokens = new Set();
  const sourceFile = ts.createSourceFile(
    fileName,
    source,
    ts.ScriptTarget.Latest,
    true,
    /\.tsx$/iu.test(fileName) ? ts.ScriptKind.TSX : ts.ScriptKind.TS,
  );
  /** @type {Map<string, ReadonlySet<string>>} */
  const symbols = new Map();
  const constantStrings = new Map();
  const declared = new Set();
  const globals = new Map([
    ['fetch', new Set(['fetch-call'])],
    ['WebSocket', new Set(['websocket'])],
    ['EventSource', new Set(['websocket'])],
    ['XMLHttpRequest', new Set(['websocket'])],
  ]);
  const moduleToken = (value) => {
    const normalized = value.startsWith('node:') ? value.slice(5) : value;
    return NETWORK_MODULES.has(normalized)
      ? normalized === 'undici'
        ? 'undici'
        : `node:${normalized}`
      : null;
  };
  const merge = (name, origins) => {
    if (origins.size === 0) return false;
    const previous = symbols.get(name) ?? new Set();
    const next = new Set([...previous, ...origins]);
    if (next.size === previous.size) return false;
    symbols.set(name, next);
    return true;
  };
  const propertyName = (expression) => {
    if (ts.isPropertyAccessExpression(expression)) return expression.name.text;
    if (ts.isElementAccessExpression(expression) && expression.argumentExpression !== undefined) {
      if (
        ts.isStringLiteral(expression.argumentExpression) ||
        ts.isNoSubstitutionTemplateLiteral(expression.argumentExpression)
      ) {
        return expression.argumentExpression.text;
      }
      if (ts.isIdentifier(expression.argumentExpression)) {
        return constantStrings.get(expression.argumentExpression.text) ?? null;
      }
    }
    return null;
  };
  const originsOf = (expression) => {
    if (ts.isParenthesizedExpression(expression)) return originsOf(expression.expression);
    if (ts.isBinaryExpression(expression))
      return new Set([...originsOf(expression.left), ...originsOf(expression.right)]);
    if (ts.isObjectLiteralExpression(expression))
      return new Set(
        expression.properties.flatMap((property) =>
          ts.isPropertyAssignment(property) ? [...originsOf(property.initializer)] : [],
        ),
      );
    if (ts.isArrayLiteralExpression(expression))
      return new Set(expression.elements.flatMap((element) => [...originsOf(element)]));
    if (ts.isIdentifier(expression))
      return (
        symbols.get(expression.text) ??
        (declared.has(expression.text) ? new Set() : globals.get(expression.text)) ??
        new Set()
      );
    if (ts.isPropertyAccessExpression(expression) || ts.isElementAccessExpression(expression)) {
      const property = propertyName(expression);
      const ownerExpression = expression.expression;
      const ownerText = ownerExpression.getText(sourceFile);
      const ownerOrigins = originsOf(ownerExpression);
      const expressionKey = expression.pos >= 0 ? expression.getText(sourceFile) : null;
      const found = new Set(expressionKey === null ? [] : (symbols.get(expressionKey) ?? []));
      if (property !== null && property.replace(/^#/u, '').toLowerCase().includes('fetch')) {
        found.add(
          ownerOrigins.has('electron-net') || ownerText === 'net'
            ? 'electron-net-fetch'
            : 'fetch-call',
        );
      }
      if (property !== null && BROWSER_NETWORK_CONSTRUCTORS.has(property)) found.add('websocket');
      if (property === 'request' && (ownerOrigins.has('electron-net') || ownerText === 'net'))
        found.add('electron-net-request');
      if (property === null && ownerOrigins.has('electron-net')) found.add('electron-net-request');
      if (property === null && [...ownerOrigins].some((item) => item.startsWith('module:socket:')))
        found.add('direct-socket-call');
      if (property === null && (ownerText === 'globalThis' || ownerText === 'window')) {
        found.add('fetch-call');
        found.add('websocket');
      }
      if (property === 'sendBeacon') found.add('send-beacon');
      if (
        property !== null &&
        SOCKET_METHODS.has(property) &&
        [...ownerOrigins].some((item) => item.startsWith('module:socket:'))
      )
        found.add('direct-socket-call');
      if (property === 'bind' || property === 'call' || property === 'apply' || found.size === 0) {
        for (const origin of ownerOrigins) found.add(origin);
      }
      return found;
    }
    if (ts.isCallExpression(expression)) {
      const args = expression.arguments;
      if (
        expression.expression.kind === ts.SyntaxKind.ImportKeyword &&
        args[0] !== undefined &&
        ts.isStringLiteral(args[0])
      ) {
        const token = moduleToken(args[0].text);
        return token === null
          ? new Set()
          : new Set([
              ['node:net', 'node:tls', 'node:http', 'node:https', 'undici'].includes(token)
                ? `module:socket:${token}`
                : `module:${token}`,
            ]);
      }
      if (
        ts.isIdentifier(expression.expression) &&
        expression.expression.text === 'require' &&
        args[0] !== undefined &&
        ts.isStringLiteral(args[0])
      ) {
        const token = moduleToken(args[0].text);
        return token === null
          ? new Set()
          : new Set([
              ['node:net', 'node:tls', 'node:http', 'node:https', 'undici'].includes(token)
                ? `module:socket:${token}`
                : `module:${token}`,
            ]);
      }
      return originsOf(expression.expression);
    }
    return new Set();
  };

  // The declaration inventory gives local symbols precedence over similarly named browser globals.
  const recordDeclarations = (node) => {
    if (ts.isVariableDeclaration(node)) {
      if (ts.isIdentifier(node.name)) declared.add(node.name.text);
      else for (const element of node.name.elements) declared.add(element.name.getText(sourceFile));
    }
    if (ts.isFunctionDeclaration(node) && node.name !== undefined) declared.add(node.name.text);
    if (ts.isParameter(node) && ts.isIdentifier(node.name)) declared.add(node.name.text);
    if (ts.isImportClause(node) && node.name !== undefined) declared.add(node.name.text);
    if (ts.isImportSpecifier(node) || ts.isNamespaceImport(node)) declared.add(node.name.text);
    ts.forEachChild(node, recordDeclarations);
  };
  recordDeclarations(sourceFile);

  // Build a conservative symbol-origin graph to a fixed point. This follows import aliases,
  // assignments, destructuring, namespace aliases, dynamic imports/requires and computed access.
  for (const statement of sourceFile.statements) {
    if (ts.isImportDeclaration(statement) && ts.isStringLiteral(statement.moduleSpecifier)) {
      const specifier = statement.moduleSpecifier.text;
      const token = moduleToken(specifier);
      if (token !== null) tokens.add(token);
      const clause = statement.importClause;
      if (
        specifier === 'electron' &&
        clause?.namedBindings !== undefined &&
        ts.isNamedImports(clause.namedBindings)
      ) {
        for (const element of clause.namedBindings.elements)
          if ((element.propertyName ?? element.name).text === 'net')
            merge(element.name.text, new Set(['electron-net']));
      }
      if (token !== null && clause?.namedBindings !== undefined) {
        const moduleOrigin =
          token === 'node:net' ||
          token === 'node:tls' ||
          token === 'node:http' ||
          token === 'node:https' ||
          token === 'undici'
            ? `module:socket:${token}`
            : `module:${token}`;
        if (ts.isNamespaceImport(clause.namedBindings))
          merge(clause.namedBindings.name.text, new Set([moduleOrigin]));
        if (ts.isNamedImports(clause.namedBindings))
          for (const element of clause.namedBindings.elements) {
            const imported = (element.propertyName ?? element.name).text;
            if (imported === 'fetch') merge(element.name.text, new Set(['fetch-call']));
            else if (SOCKET_METHODS.has(imported))
              merge(element.name.text, new Set(['direct-socket-call']));
          }
      }
    }
  }
  for (let pass = 0; pass < 12; pass += 1) {
    let changed = false;
    const bind = (name, initializer) => {
      if (ts.isIdentifier(name)) changed = merge(name.text, originsOf(initializer)) || changed;
      else if (ts.isObjectBindingPattern(name))
        for (const element of name.elements) {
          const property = (element.propertyName ?? element.name)
            .getText(sourceFile)
            .replaceAll(/["']/gu, '');
          const synthetic = ts.factory.createElementAccessExpression(
            initializer,
            ts.factory.createStringLiteral(property),
          );
          changed = merge(element.name.getText(sourceFile), originsOf(synthetic)) || changed;
        }
    };
    const collect = (node) => {
      if (ts.isVariableDeclaration(node) && node.initializer !== undefined) {
        bind(node.name, node.initializer);
        if (ts.isIdentifier(node.name)) {
          const value =
            ts.isStringLiteral(node.initializer) ||
            ts.isNoSubstitutionTemplateLiteral(node.initializer)
              ? node.initializer.text
              : ts.isIdentifier(node.initializer)
                ? constantStrings.get(node.initializer.text)
                : undefined;
          if (value !== undefined && constantStrings.get(node.name.text) !== value) {
            constantStrings.set(node.name.text, value);
            changed = true;
          }
        }
        if (ts.isIdentifier(node.name) && ts.isObjectLiteralExpression(node.initializer)) {
          for (const property of node.initializer.properties) {
            if (!ts.isPropertyAssignment(property)) continue;
            const name =
              propertyName(property.name) ??
              property.name.getText(sourceFile).replaceAll(/["']/gu, '');
            changed =
              merge(`${node.name.text}.${name}`, originsOf(property.initializer)) || changed;
          }
        }
        if (ts.isObjectBindingPattern(node.name))
          for (const element of node.name.elements) {
            const property = (element.propertyName ?? element.name).getText(sourceFile);
            const owner = node.initializer.getText(sourceFile);
            if (owner === 'net' && property === 'request')
              changed =
                merge(element.name.getText(sourceFile), new Set(['electron-net-request'])) ||
                changed;
            if (property === 'fetch')
              changed = merge(element.name.getText(sourceFile), new Set(['fetch-call'])) || changed;
          }
      }
      if (
        ts.isBinaryExpression(node) &&
        node.operatorToken.kind === ts.SyntaxKind.EqualsToken &&
        (ts.isIdentifier(node.left) ||
          ts.isPropertyAccessExpression(node.left) ||
          ts.isElementAccessExpression(node.left))
      ) {
        changed = merge(node.left.getText(sourceFile), originsOf(node.right)) || changed;
        if (ts.isIdentifier(node.left)) {
          const value =
            ts.isStringLiteral(node.right) || ts.isNoSubstitutionTemplateLiteral(node.right)
              ? node.right.text
              : ts.isIdentifier(node.right)
                ? constantStrings.get(node.right.text)
                : undefined;
          if (value === undefined) constantStrings.delete(node.left.text);
          else if (constantStrings.get(node.left.text) !== value) {
            constantStrings.set(node.left.text, value);
            changed = true;
          }
        }
      }
      if (ts.isFunctionDeclaration(node) && node.name !== undefined && node.body !== undefined) {
        for (const statement of node.body.statements) {
          if (ts.isReturnStatement(statement) && statement.expression !== undefined) {
            changed = merge(node.name.text, originsOf(statement.expression)) || changed;
          }
        }
      }
      ts.forEachChild(node, collect);
    };
    collect(sourceFile);
    if (!changed) break;
  }
  const visit = (node) => {
    if (
      ts.isVariableDeclaration(node) &&
      ts.isObjectBindingPattern(node.name) &&
      node.initializer !== undefined
    ) {
      const owner = node.initializer.getText(sourceFile);
      for (const element of node.name.elements) {
        const property = (element.propertyName ?? element.name).getText(sourceFile);
        if (owner === 'net' && property === 'request') tokens.add('electron-net-request');
        if (property === 'fetch') tokens.add('fetch-call');
      }
    }
    if (
      ts.isExportDeclaration(node) &&
      node.moduleSpecifier !== undefined &&
      ts.isStringLiteral(node.moduleSpecifier)
    ) {
      const token = moduleToken(node.moduleSpecifier.text);
      if (token !== null) tokens.add(token);
    }
    if (ts.isCallExpression(node) || ts.isNewExpression(node)) {
      if (
        ts.isCallExpression(node) &&
        node.expression.kind === ts.SyntaxKind.ImportKeyword &&
        node.arguments[0] !== undefined &&
        ts.isStringLiteral(node.arguments[0])
      ) {
        const token = moduleToken(node.arguments[0].text);
        if (token !== null) tokens.add(token);
      }
      for (const origin of originsOf(node.expression))
        if (!origin.startsWith('module:') && origin !== 'electron-net') tokens.add(origin);
      for (const argument of node.arguments ?? [])
        for (const origin of originsOf(argument))
          if (!origin.startsWith('module:') && origin !== 'electron-net') tokens.add(origin);
      if (
        ts.isCallExpression(node) &&
        ts.isIdentifier(node.expression) &&
        node.expression.text === 'requireRecord' &&
        node.arguments[1] !== undefined &&
        ts.isStringLiteral(node.arguments[1])
      ) {
        const token = moduleToken(node.arguments[1].text);
        if (token !== null) tokens.add(token);
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(sourceFile);
  return Object.freeze([...tokens].sort());
}

export async function verifyNetworkBoundary(root = SOURCE_ROOT) {
  const findings = [];
  for (const absolute of await walk(root)) {
    if (!/\.(?:ts|tsx|js|cjs|mjs)$/u.test(absolute)) continue;
    const source = await readFile(absolute, 'utf8');
    const path = normalize(relative(ROOT, absolute));
    findings.push(...detectNetworkTokens(source, path).map((token) => ({ path, token })));
  }

  const unexpected = findings.filter(({ path, token }) => {
    const approval = APPROVED_NETWORK_BOUNDARIES[path];
    return approval === undefined || !approval.tokens.includes(token);
  });
  if (unexpected.length > 0) {
    throw new Error(
      `Unapproved direct networking boundary:\n${unexpected.map(({ path, token }) => `- ${path}: ${token}`).join('\n')}`,
    );
  }
  for (const [path, approval] of Object.entries(APPROVED_NETWORK_BOUNDARIES)) {
    const seen = new Set(findings.filter((item) => item.path === path).map((item) => item.token));
    const missing = approval.tokens.filter((token) => !seen.has(token));
    if (missing.length > 0)
      throw new Error(`Stale network approval for ${path}: ${missing.join(', ')}`);
  }
  return Object.freeze(
    Object.entries(APPROVED_NETWORK_BOUNDARIES).map(([path, value]) =>
      Object.freeze({ path, category: value.category, reason: value.reason }),
    ),
  );
}

async function walk(directory) {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const absolute = resolve(directory, entry.name);
    if (entry.isDirectory()) files.push(...(await walk(absolute)));
    else if (entry.isFile()) files.push(absolute);
  }
  return files.sort();
}

function normalize(path) {
  return path.replaceAll('\\', '/');
}

if (process.argv[1] !== undefined && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const inventory = await verifyNetworkBoundary();
  console.log(`Closed networking boundary verified (${inventory.length} approved choke points).`);
  for (const item of inventory) console.log(`- ${item.category}: ${item.path}`);
}
