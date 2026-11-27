// One page per tag of api/chat-api.yaml. The contract names no tags, so every operation falls under one default tag.
import { usePaths } from "vitepress-openapi";
import { DEFAULT_TAG, loadSpec } from "../../.vitepress/openapi/spec";

export default {
  paths() {
    return usePaths({ spec: loadSpec(), defaultTag: DEFAULT_TAG })
      .getTags()
      .map(({ name }: any) => ({ params: { tag: name, pageTitle: name } }));
  },
};
