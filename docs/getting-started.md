# Run it locally

The local stack runs the orchestrator, the web chat surface, a Postgres and scripted mock agents. Start at
[dev/README.md](https://github.com/vymalo/another-agentic-system/blob/main/dev/README.md), section "Test it locally".

```sh
docker compose --profile app up -d --wait
sh dev/greeting-e2e.sh
```

Read [the vision](vision.md) for what the system is meant to be and [the MVP](mvp.md) for the build order.
