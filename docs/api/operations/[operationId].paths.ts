// One page per operation of api/chat-api.yaml, found by its operationId.
import { usePaths } from "vitepress-openapi";
import { assertRouteId } from "../../.vitepress/openapi/shared";
import { DEFAULT_TAG, loadSpec } from "../../.vitepress/openapi/spec";

export default {
  paths() {
    return usePaths({ spec: loadSpec(), defaultTag: DEFAULT_TAG })
      .getPathsByVerbs()
      .map(({ operationId, summary, verb, path }: any) => ({
        params: { operationId: assertRouteId(operationId, `operationId of ${verb} ${path}`), pageTitle: `${summary ?? `${verb.toUpperCase()} ${path}`}` },
      }));
  },
};
