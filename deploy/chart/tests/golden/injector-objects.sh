#!/bin/sh
# injector-objects.sh: from a render on stdin, every object the model-credential injector adds (values.yaml, `injector`):
# the Gateway, the EnvoyProxy that pins its ClusterIP Service name, the client traffic policy, the upstream Backend and
# its AIServiceBackend, the AIGatewayRoute, the Authorino instance, the AuthConfig, the ext-auth SecurityPolicy, the
# balance EnvoyExtensionPolicy and the injector's broker-token and ledger shared-secret ExternalSecrets. Without the two labels that move with the chart's own version. What
# render-check.sh compares with tests/golden/injector.yaml; see there for how a golden is made.
awk '
  function flush() {
    if (buf ~ /app\.kubernetes\.io\/component: injector\n/) printf "---\n%s", buf
    buf = ""
  }
  /^---$/ { flush(); next }
  /^    (helm\.sh\/chart|app\.kubernetes\.io\/version):/ { next }
  { buf = buf $0 "\n" }
  END { flush() }'
