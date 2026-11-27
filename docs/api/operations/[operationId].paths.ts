// One page per operation of api/chat-api.yaml, found by its operationId.
import { usePaths } from "vitepress-openapi";
import { DEFAULT_TAG, loadSpec } from "../../.vitepress/openapi/spec";

export default {
  paths() {
    return usePaths({ spec: loadSpec(), defaultTag: DEFAULT_TAG })
      .getPathsByVerbs()
      .map(({ operationId, summary, verb, path }: any) => ({
        params: { operationId, pageTitle: `${summary ?? `${verb.toUpperCase()} ${path}`}` },
      }));
  },
};
