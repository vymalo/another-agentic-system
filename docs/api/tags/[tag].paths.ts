// One page per tag of api/chat-api.yaml. The contract names no tags, so every operation falls under one default tag.
import { usePaths } from "vitepress-openapi";
import { assertRouteId } from "../../.vitepress/openapi/shared";
import { DEFAULT_TAG, loadSpec } from "../../.vitepress/openapi/spec";

export default {
  paths() {
    return usePaths({ spec: loadSpec(), defaultTag: DEFAULT_TAG })
      .getTags()
      .map(({ name }: any) => ({ params: { tag: assertRouteId(name, "tag"), pageTitle: name } }));
  },
};
