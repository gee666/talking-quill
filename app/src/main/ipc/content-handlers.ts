import type { HandlerDependencies } from './handler-dependencies';
import type { InvokeHandlerMap } from './types';

type ContentHandlerDependencies = Pick<
  HandlerDependencies,
  | 'commands'
  | 'history'
  | 'models'
  | 'settingsTransferFiles'
  | 'vocabulary'
  | 'vocabularyFiles'
  | 'windows'
>;
type ContentHandlers = Pick<
  InvokeHandlerMap,
  | 'history:list'
  | 'history:delete'
  | 'history:delete-all'
  | 'history:copy'
  | 'history:thumbnail'
  | 'model:list'
  | 'model:status'
  | 'model:download'
  | 'model:pause'
  | 'model:cancel'
  | 'model:retry'
  | 'model:delete'
  | 'commands:list'
  | 'commands:create'
  | 'commands:update'
  | 'commands:delete'
  | 'commands:preview'
  | 'commands:import-file'
  | 'commands:export-file'
  | 'vocabulary:list'
  | 'vocabulary:create'
  | 'vocabulary:update'
  | 'vocabulary:delete'
  | 'vocabulary:import-file'
  | 'vocabulary:export-file'
>;

export function createContentHandlers(dependencies: ContentHandlerDependencies): ContentHandlers {
  return {
    'history:list': (request) => dependencies.history.list(request),
    'history:delete': ({ id }) => dependencies.history.deleteById(id),
    'history:delete-all': () => dependencies.history.deleteAll(),
    'history:copy': ({ id }) => dependencies.history.copy(id),
    'history:thumbnail': ({ id }) => dependencies.history.thumbnail(id),
    'model:list': () => dependencies.models.list(),
    'model:status': ({ modelId, verify }) => dependencies.models.status(modelId, verify),
    'model:download': ({ modelId }) => dependencies.models.download(modelId),
    'model:pause': ({ modelId }) => dependencies.models.pause(modelId),
    'model:cancel': ({ modelId }) => dependencies.models.cancel(modelId),
    'model:retry': ({ modelId }) => dependencies.models.retry(modelId),
    'model:delete': ({ modelId }) => dependencies.models.deleteIfIdle(modelId),
    'commands:list': () => [...dependencies.commands.list()],
    'commands:create': (input) => dependencies.commands.create(input),
    'commands:update': ({ id, patch }) => dependencies.commands.update(id, patch),
    'commands:delete': async ({ id }) => ({ deleted: await dependencies.commands.delete(id) }),
    'commands:preview': ({ transcript }) => dependencies.commands.match(transcript),
    'commands:import-file': (_request, context) => {
      const owner = dependencies.windows.getByWebContentsId(context.webContentsId);
      if (owner === null) throw new Error('Voice command dialog owner is unavailable');
      return dependencies.settingsTransferFiles.importVoiceCommands(owner);
    },
    'commands:export-file': (_request, context) => {
      const owner = dependencies.windows.getByWebContentsId(context.webContentsId);
      if (owner === null) throw new Error('Voice command dialog owner is unavailable');
      return dependencies.settingsTransferFiles.exportVoiceCommands(owner);
    },
    'vocabulary:list': () => [...dependencies.vocabulary.list()],
    'vocabulary:create': ({ value }) => dependencies.vocabulary.create(value),
    'vocabulary:update': ({ id, value }) => dependencies.vocabulary.update(id, value),
    'vocabulary:delete': async ({ id }) => ({ deleted: await dependencies.vocabulary.delete(id) }),
    'vocabulary:import-file': (_request, context) => {
      const owner = dependencies.windows.getByWebContentsId(context.webContentsId);
      if (owner === null) throw new Error('Vocabulary dialog owner is unavailable');
      return dependencies.vocabularyFiles.importFile(owner);
    },
    'vocabulary:export-file': (_request, context) => {
      const owner = dependencies.windows.getByWebContentsId(context.webContentsId);
      if (owner === null) throw new Error('Vocabulary dialog owner is unavailable');
      return dependencies.vocabularyFiles.exportFile(owner);
    },
  };
}
