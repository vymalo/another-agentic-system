// One page per tag of api/chat-api.yaml (its top-level `tags:`; every operation has exactly one). Tag names are one word each,
// so they are safe route segments as they are (assertRouteId).
import { usePaths } from "vitepress-openapi";
import { assertRouteId } from "../../.vitepress/openapi/shared";
import { loadSpec } from "../../.vitepress/openapi/spec";

export default {
  paths() {
    return usePaths({ spec: loadSpec() })
      .getTags()
      .map(({ name }: any) => ({ params: { tag: assertRouteId(name, "tag"), pageTitle: name } }));
  },
};
