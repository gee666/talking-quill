import { statSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { dirname, extname, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const canonicalEntry = resolve(repositoryRoot, 'app/src/main/index.ts');
const forbiddenSymbols = Object.freeze([
  'runExtension',
  'ApplicationRuntimeExtension',
  'verifyWriteFailureContainment',
  'probeInstalledAcceptanceWriteFailure',
  'acceptance-injected-write-failure',
  'requestExtension',
  'requestAcceptance',
]);

export async function verifyCanonicalMainGraph(entry = canonicalEntry) {
  const graph = await collectStaticGraph(resolve(entry));
  const violations = [];
  for (const [path, source] of graph) {
    const name = relative(repositoryRoot, path).replaceAll('\\', '/');
    if (name.includes('/main/acceptance/') || name.includes('/main/entries/')) {
      violations.push(`${name}: acceptance-only module is reachable`);
    }
    for (const symbol of forbiddenSymbols) {
      if (source.includes(symbol)) violations.push(`${name}: forbidden symbol ${symbol}`);
    }
  }

  const helperClientPath = resolve(repositoryRoot, 'app/src/main/helper/helper-client.ts');
  const helperChannelPath = resolve(repositoryRoot, 'app/src/main/helper/helper-rpc-channel.ts');
  const [helperClient, helperChannel] = await Promise.all([
    readFile(helperClientPath, 'utf8'),
    readFile(helperChannelPath, 'utf8'),
  ]);
  try {
    verifyCanonicalHelperTransportSources(helperClient, helperChannel);
  } catch (error) {
    violations.push(error instanceof Error ? error.message : String(error));
  }
  if (violations.length > 0) {
    throw new Error(`Canonical main graph policy failed:\n${violations.join('\n')}`);
  }
  return Object.freeze(
    [...graph.keys()].map((path) => relative(repositoryRoot, path).replaceAll('\\', '/')).sort(),
  );
}

export function verifyCanonicalHelperTransportSources(helperClient, helperChannel) {
  const violations = [];
  const clientFile = ts.createSourceFile(
    'helper-client.ts',
    helperClient,
    ts.ScriptTarget.Latest,
    true,
  );
  const channelFile = ts.createSourceFile(
    'helper-rpc-channel.ts',
    helperChannel,
    ts.ScriptTarget.Latest,
    true,
  );
  const channelClass = findClass(channelFile, 'HelperRpcChannel');
  const dispatchers = methodsReachingFrameEncoder(channelClass);
  const publicDispatchers = dispatchers.filter((method) => !methodIsPrivate(method));
  if (
    publicDispatchers.length !== 1 ||
    methodName(publicDispatchers[0]) !== 'request' ||
    !isTypedHelperRequest(publicDispatchers[0])
  ) {
    violations.push(
      'helper-rpc-channel.ts: RPC dispatch must have one public HelperMethod-typed request method',
    );
  }

  const pending = channelFile.statements.find(
    (statement) => ts.isInterfaceDeclaration(statement) && statement.name.text === 'PendingRequest',
  );
  const pendingMethod =
    pending === undefined || !ts.isInterfaceDeclaration(pending)
      ? undefined
      : pending.members.find(
          (member) =>
            ts.isPropertySignature(member) &&
            member.name !== undefined &&
            member.name.getText(channelFile) === 'method',
        );
  if (
    pendingMethod === undefined ||
    !ts.isPropertySignature(pendingMethod) ||
    pendingMethod.type?.getText(channelFile) !== 'HelperMethod'
  ) {
    violations.push('helper-rpc-channel.ts: pending RPC method must be HelperMethod');
  }
  if (!helperChannel.includes('helperResultSchemas[pending.method].safeParse')) {
    violations.push(
      'helper-rpc-channel.ts: result schema must be selected by typed pending method',
    );
  }

  const clientClass = findClass(clientFile, 'HelperClient');
  const request = clientClass.members.find(
    (member) => ts.isMethodDeclaration(member) && methodName(member) === 'request',
  );
  if (request === undefined || !ts.isMethodDeclaration(request) || !isTypedHelperRequest(request)) {
    violations.push('helper-client.ts: canonical request must be HelperMethod-typed');
  }
  for (const member of clientClass.members) {
    if (!ts.isMethodDeclaration(member) || member.body === undefined) continue;
    const name = methodName(member);
    inspectCalls(member.body, (call) => {
      if (isThisMethodCall(call, 'request') && name !== 'request') {
        const [method] = call.arguments;
        if (method === undefined || !ts.isStringLiteral(method)) {
          violations.push(
            `helper-client.ts: ${name} forwards a nonliteral method to the RPC dispatcher`,
          );
        }
      }
      if (isRpcChannelRequest(call) && name !== 'request') {
        const method = call.arguments[1];
        if (method === undefined || !ts.isStringLiteral(method)) {
          violations.push(
            `helper-client.ts: ${name} sends a nonliteral method through the RPC channel`,
          );
        }
      }
    });
  }
  if (violations.length > 0) throw new Error(violations.join('\n'));
}

function findClass(sourceFile, name) {
  const declaration = sourceFile.statements.find(
    (statement) => ts.isClassDeclaration(statement) && statement.name?.text === name,
  );
  if (declaration === undefined || !ts.isClassDeclaration(declaration)) {
    throw new Error(`${name} declaration is missing`);
  }
  return declaration;
}

function methodsReachingFrameEncoder(declaration) {
  const methods = declaration.members.filter(ts.isMethodDeclaration);
  const calls = new Map(
    methods.map((method) => {
      const names = new Set();
      if (method.body !== undefined) {
        inspectCalls(method.body, (call) => {
          const name = calledThisMethodName(call);
          if (name !== null) names.add(name);
        });
      }
      return [methodName(method), names];
    }),
  );
  const sinks = new Set(
    methods
      .filter((method) => method.body?.getText().includes('encodeHelperFrame(') === true)
      .map(methodName),
  );
  let changed = true;
  while (changed) {
    changed = false;
    for (const [name, dependencies] of calls) {
      if (!sinks.has(name) && [...dependencies].some((dependency) => sinks.has(dependency))) {
        sinks.add(name);
        changed = true;
      }
    }
  }
  return methods.filter((method) => sinks.has(methodName(method)));
}

function isTypedHelperRequest(method) {
  const typeParameter = method.typeParameters?.find(
    (parameter) =>
      parameter.name.text === 'Method' && parameter.constraint?.getText() === 'HelperMethod',
  );
  const methodParameter = method.parameters.find(
    (parameter) => parameter.name.getText() === 'method',
  );
  const paramsParameter = method.parameters.find(
    (parameter) => parameter.name.getText() === 'params',
  );
  return (
    typeParameter !== undefined &&
    methodParameter?.type?.getText() === 'Method' &&
    paramsParameter?.type?.getText() === 'HelperParams<Method>'
  );
}

function methodIsPrivate(method) {
  return (
    ts.isPrivateIdentifier(method.name) ||
    method.modifiers?.some((modifier) => modifier.kind === ts.SyntaxKind.PrivateKeyword) === true
  );
}

function methodName(method) {
  return ts.isPrivateIdentifier(method.name) || ts.isIdentifier(method.name)
    ? method.name.text
    : method.name.getText();
}

function inspectCalls(node, inspect) {
  function visit(child) {
    if (ts.isCallExpression(child)) inspect(child);
    ts.forEachChild(child, visit);
  }
  visit(node);
}

function calledThisMethodName(call) {
  if (
    !ts.isPropertyAccessExpression(call.expression) ||
    call.expression.expression.kind !== ts.SyntaxKind.ThisKeyword
  ) {
    return null;
  }
  return call.expression.name.text;
}

function isThisMethodCall(call, name) {
  return calledThisMethodName(call) === name;
}

function isRpcChannelRequest(call) {
  return (
    ts.isPropertyAccessExpression(call.expression) &&
    call.expression.name.text === 'request' &&
    call.expression.expression.getText().includes('#rpcChannel')
  );
}

async function collectStaticGraph(entry) {
  const graph = new Map();
  async function visit(path) {
    if (graph.has(path)) return;
    const source = await readFile(path, 'utf8');
    graph.set(path, source);
    if (!['.ts', '.tsx'].includes(extname(path))) return;
    const sourceFile = ts.createSourceFile(path, source, ts.ScriptTarget.Latest, true);
    const specifiers = [];
    function inspect(node) {
      if (
        (ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) &&
        node.moduleSpecifier !== undefined &&
        ts.isStringLiteral(node.moduleSpecifier)
      ) {
        specifiers.push(node.moduleSpecifier.text);
      } else if (
        ts.isImportEqualsDeclaration(node) &&
        ts.isExternalModuleReference(node.moduleReference) &&
        node.moduleReference.expression !== undefined &&
        ts.isStringLiteral(node.moduleReference.expression)
      ) {
        specifiers.push(node.moduleReference.expression.text);
      } else if (ts.isCallExpression(node)) {
        const dynamicImport = node.expression.kind === ts.SyntaxKind.ImportKeyword;
        const commonJsRequire =
          ts.isIdentifier(node.expression) && node.expression.text === 'require';
        if (dynamicImport || commonJsRequire) {
          const [argument] = node.arguments;
          if (argument === undefined || !ts.isStringLiteral(argument)) {
            throw new Error(
              `Canonical graph has a nonliteral module load: ${relative(repositoryRoot, path)}`,
            );
          }
          specifiers.push(argument.text);
        }
      }
      ts.forEachChild(node, inspect);
    }
    inspect(sourceFile);
    for (const specifier of specifiers) {
      if (!specifier.startsWith('.')) continue;
      const dependency = resolveTypeScriptModule(dirname(path), specifier);
      if (dependency === null) {
        throw new Error(
          `Canonical graph could not resolve local module ${specifier} from ${relative(repositoryRoot, path)}`,
        );
      }
      await visit(dependency);
    }
  }
  await visit(entry);
  return graph;
}

function resolveTypeScriptModule(directory, specifier) {
  const base = resolve(directory, specifier);
  for (const candidate of [
    base,
    `${base}.ts`,
    `${base}.tsx`,
    `${base}.json`,
    resolve(base, 'index.ts'),
    resolve(base, 'index.tsx'),
  ]) {
    try {
      if (statSync(candidate).isFile()) return candidate;
    } catch {
      // Try the next supported local module shape.
    }
  }
  return null;
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  const files = await verifyCanonicalMainGraph();
  console.log(`Canonical main import graph verified (${String(files.length)} modules)`);
}
