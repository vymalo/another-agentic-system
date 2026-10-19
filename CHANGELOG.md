# Changelog

## [0.7.1](https://github.com/vymalo/another-agentic-system/compare/v0.7.0...v0.7.1) (2026-10-19)


### Documentation

* a public roadmap, and how to contribute and report security issues ([#298](https://github.com/vymalo/another-agentic-system/issues/298)) ([ce39839](https://github.com/vymalo/another-agentic-system/commit/ce39839cecea81c22202f34b8b93706251a380f2))

## [0.7.0](https://github.com/vymalo/another-agentic-system/compare/v0.6.0...v0.7.0) (2026-10-18)


### Features

* **asks:** an asked agent is told what the person said in the thread ([#271](https://github.com/vymalo/another-agentic-system/issues/271)) ([db42511](https://github.com/vymalo/another-agentic-system/commit/db42511b670ac671c474cc3cd4afe4b75aa2f051))


### Bug Fixes

* **asks:** an ask is never sent without its grant, and an asked agent gets a forms-only catalog ([#270](https://github.com/vymalo/another-agentic-system/issues/270)) ([591fc28](https://github.com/vymalo/another-agentic-system/commit/591fc280fb9b98d28bd743449c9fe2713ddf7533))
* **tiers:** the role cap holds at every send and ask, and an asked agent's shown form ends the ask ([#267](https://github.com/vymalo/another-agentic-system/issues/267)) ([d774cd9](https://github.com/vymalo/another-agentic-system/commit/d774cd9b1ee3a2ab2b35fcf24e6267146df76548))


### Build System

* **adam:** bump adam-rs to 8847c28 (access tiers, Questions v5 forms, asked-coder consent) ([#269](https://github.com/vymalo/another-agentic-system/issues/269)) ([64d230d](https://github.com/vymalo/another-agentic-system/commit/64d230d1bfd42db13a8f737d9adc1522730fec9d))


### Chores

* **adam:** bump adam-rs to ddef35e (an asked coder hears the person only through person.said) ([#272](https://github.com/vymalo/another-agentic-system/issues/272)) ([3f7bda0](https://github.com/vymalo/another-agentic-system/commit/3f7bda098029cecbe5f31e1246210b8bb3065ced))
* **deploy:** bump orchestrator to sha-591fc28 ([cbaf6e1](https://github.com/vymalo/another-agentic-system/commit/cbaf6e14d31c4a6732af9c6b5f5d80db56a15acf))
* **deploy:** bump orchestrator to sha-64d230d ([522b535](https://github.com/vymalo/another-agentic-system/commit/522b5358ae233142f307f7ba11c4d93704eb2c96))
* **deploy:** bump orchestrator to sha-d774cd9 ([cb8483c](https://github.com/vymalo/another-agentic-system/commit/cb8483c32535e906e77d1e0e3d9232e246947a38))
* **deploy:** bump orchestrator to sha-db42511 ([4dbc5d2](https://github.com/vymalo/another-agentic-system/commit/4dbc5d2f71c095e48134936bc612aabcf9dfcfd9))
* **deploy:** bump web to sha-d774cd9 ([a8457e8](https://github.com/vymalo/another-agentic-system/commit/a8457e8a642f4e7953176dd9ac949ff267338b62))

## [0.6.0](https://github.com/vymalo/another-agentic-system/compare/v0.5.0...v0.6.0) (2026-10-17)


### Features

* **chart:** continuous backups of the database with the barman-cloud plugin, off by default ([#151](https://github.com/vymalo/another-agentic-system/issues/151)) ([#264](https://github.com/vymalo/another-agentic-system/issues/264)) ([9e682b2](https://github.com/vymalo/another-agentic-system/commit/9e682b2a2afda10f93a4608bcf165b5fa787373d))
* **orchestrator:** a platform agent's audience decides who may use it (ADR 0045, D8) ([#263](https://github.com/vymalo/another-agentic-system/issues/263)) ([8df44c6](https://github.com/vymalo/another-agentic-system/commit/8df44c6f88f0d9eb47aa7c9f5874a914765c8376))


### Bug Fixes

* **auth-jwt:** never hold the key lock across a fetch; serve a stale copy while refreshing ([#262](https://github.com/vymalo/another-agentic-system/issues/262)) ([da4056d](https://github.com/vymalo/another-agentic-system/commit/da4056db6aea1230fe46b0eb0154e9b861a6b494))
* **auth-jwt:** read every valid Ed25519, RSA and P-256 key; check iss and the user strictly ([#261](https://github.com/vymalo/another-agentic-system/issues/261)) ([b630adf](https://github.com/vymalo/another-agentic-system/commit/b630adff3b9bc91780a6ac05ae5af062a04ad3ff))
* **orchestrator:** lock hyper 1.12.0 and want 0.3.2, and guard the lock against going back ([#258](https://github.com/vymalo/another-agentic-system/issues/258)) ([8af4f74](https://github.com/vymalo/another-agentic-system/commit/8af4f7490b45373a644460fe8882f8f18aa8f927))
* **orchestrator:** lock rustls 0.23.46 for RUSTSEC-2026-0346 to 0349 ([#260](https://github.com/vymalo/another-agentic-system/issues/260)) ([1525e81](https://github.com/vymalo/another-agentic-system/commit/1525e81f3705a40c80b3b6e5adf8b33d73f3e8b3))


### Documentation

* the platform's registry is read by this orchestrator in the platform's CI ([#266](https://github.com/vymalo/another-agentic-system/issues/266)) ([33645d1](https://github.com/vymalo/another-agentic-system/commit/33645d199438507d2696e86012906c6f26874d70))


### Tests

* **chart:** a kind end-to-end backs up the database and restores it ([#151](https://github.com/vymalo/another-agentic-system/issues/151)) ([#265](https://github.com/vymalo/another-agentic-system/issues/265)) ([a2be5e2](https://github.com/vymalo/another-agentic-system/commit/a2be5e28e325e290084a7e572bd24f201d123722))


### Chores

* **deploy:** bump orchestrator to sha-1525e81 ([f367e0f](https://github.com/vymalo/another-agentic-system/commit/f367e0f551917b5803633482947c18c02b0a0624))
* **deploy:** bump orchestrator to sha-8af4f74 ([cec9268](https://github.com/vymalo/another-agentic-system/commit/cec926856b9e08d19b428972c28c8ff97a4f479d))
* **deploy:** bump orchestrator to sha-8df44c6 ([66bea6c](https://github.com/vymalo/another-agentic-system/commit/66bea6cf245e61430373a8df6983f11ae0599f3a))
* **deploy:** bump orchestrator to sha-b630adf ([58cca23](https://github.com/vymalo/another-agentic-system/commit/58cca23e0604b727088112ba6554e9f25b264b35))
* **deploy:** bump web to sha-8df44c6 ([2120423](https://github.com/vymalo/another-agentic-system/commit/2120423325ac12c3bb5f6525af527dd437fbed84))

## [0.5.0](https://github.com/vymalo/another-agentic-system/compare/v0.4.3...v0.5.0) (2026-10-15)


### Features

* **api:** add, list, edit and delete people's own MCP servers (ADR 0046, slice 4b) ([#254](https://github.com/vymalo/another-agentic-system/issues/254)) ([34e5ef8](https://github.com/vymalo/another-agentic-system/commit/34e5ef8cdb127d65a2f98eb272e5446fcf1182ad))
* **api:** authorize a person's own oauth2 MCP server, its callback and client document (ADR 0046, 4c) ([#255](https://github.com/vymalo/another-agentic-system/issues/255)) ([5d967b9](https://github.com/vymalo/another-agentic-system/commit/5d967b9a8922146bd3e3027c80d720ceae6d07b3))
* **credential-broker:** the CredentialBroker with OAuth and the egress guard (ADR 0046, slice 2) ([#251](https://github.com/vymalo/another-agentic-system/issues/251)) ([804b67a](https://github.com/vymalo/another-agentic-system/commit/804b67af7ca0a66abaf7c17d8f073da4c35e5011))
* **deploy:** the public files host of RustFS, a Caddy proxy behind a Traefik Ingress (ADR 0058 A5.2) ([#247](https://github.com/vymalo/another-agentic-system/issues/247)) ([d2ca24e](https://github.com/vymalo/another-agentic-system/commit/d2ca24eeadd19137fe5ddc4433090967e3fa8628))
* downloads by a 302 to a presigned GET, opt-in by artifacts.s3.downloads (ADR 0058 A1) ([#246](https://github.com/vymalo/another-agentic-system/issues/246)) ([1dc6c21](https://github.com/vymalo/another-agentic-system/commit/1dc6c2151afdb3672a40c17db956c37430ba2b56))
* large files uploaded by an agent through a presigned URL, the orchestrator's side (ADR 0058, uploads/v1) ([#245](https://github.com/vymalo/another-agentic-system/issues/245)) ([312ad2a](https://github.com/vymalo/another-agentic-system/commit/312ad2a05631805b656f022b5aff9fb3b9b0c8ce))
* orchestrator artifacts migrate --from fs --to s3, and the chart Job that runs it (ADR 0058 A5.4) ([#248](https://github.com/vymalo/another-agentic-system/issues/248)) ([2a0cc6e](https://github.com/vymalo/another-agentic-system/commit/2a0cc6e348ac0f3f124fc93335e662618498900f))
* **orchestrator:** people's MCP server definitions, config and wiring (ADR 0046, slice 4a) ([#253](https://github.com/vymalo/another-agentic-system/issues/253)) ([bfafb08](https://github.com/vymalo/another-agentic-system/commit/bfafb08f7cfd7c7b15caa54da9eb44a72322af71))
* **ports:** the ports of a person's MCP servers and their secrets, and the egress guard (ADR 0046, slice 1) ([#250](https://github.com/vymalo/another-agentic-system/issues/250)) ([7ca5e05](https://github.com/vymalo/another-agentic-system/commit/7ca5e0506876336ade847f03d538cb3166348ce8))
* **relay:** attach a person's own MCP server and relay it with a lease per call (ADR 0046, 5a) ([#256](https://github.com/vymalo/another-agentic-system/issues/256)) ([1612f61](https://github.com/vymalo/another-agentic-system/commit/1612f612e7e20e2f5fe696c33b810f73fd2c9fe0))
* **secrets-pg:** the sealed SecretStore in Postgres, its key and rewrap (ADR 0046, slice 3) ([#252](https://github.com/vymalo/another-agentic-system/issues/252)) ([22b6f9b](https://github.com/vymalo/another-agentic-system/commit/22b6f9b695a704b5e2b32ca382f3f5162dd2c0c3))
* **web:** the owner's thread list (Pinned, Chats, Archived, row menu) and thread deletion ([#242](https://github.com/vymalo/another-agentic-system/issues/242)) ([7f7c596](https://github.com/vymalo/another-agentic-system/commit/7f7c59648ec5718e1fd84a058b9e86c57f2725e3))


### Bug Fixes

* **agui:** a shared reader gets the link's file route, the stream says where the log ends, and the agent's name ([#249](https://github.com/vymalo/another-agentic-system/issues/249)) ([6fbd910](https://github.com/vymalo/another-agentic-system/commit/6fbd910c2ea90c8aba4661cb7a89c7a4c29d2906))


### Tests

* **dev:** an agent calls a person's own MCP server through the relay (ADR 0046, 5b) ([#257](https://github.com/vymalo/another-agentic-system/issues/257)) ([3e0bb1e](https://github.com/vymalo/another-agentic-system/commit/3e0bb1e92990eeb74922b4797edd4d6c997c6cb3))
* **dev:** share-e2e.sh through the real edge, the dev carve-outs, and the chart's routes that skip sign-in ([#244](https://github.com/vymalo/another-agentic-system/issues/244)) ([0f50cd5](https://github.com/vymalo/another-agentic-system/commit/0f50cd5c987bb98401ddce21da9874a0dcdc6b18))


### Chores

* **deploy:** bump orchestrator to sha-1612f61 ([426e633](https://github.com/vymalo/another-agentic-system/commit/426e6335104dd960efac724880af6ff836ff2e85))
* **deploy:** bump orchestrator to sha-1dc6c21 ([91c3b1a](https://github.com/vymalo/another-agentic-system/commit/91c3b1abaee230eb0f0a113a8aaf62d948d6f88e))
* **deploy:** bump orchestrator to sha-22b6f9b ([2dd2614](https://github.com/vymalo/another-agentic-system/commit/2dd2614233438a6617622c0a5484d98369e796e6))
* **deploy:** bump orchestrator to sha-2a0cc6e ([9ba9211](https://github.com/vymalo/another-agentic-system/commit/9ba921186366d4079aefa297df150e38ee0b399f))
* **deploy:** bump orchestrator to sha-312ad2a ([6171bf0](https://github.com/vymalo/another-agentic-system/commit/6171bf082281cc5a73ae38ee024353069dee79f5))
* **deploy:** bump orchestrator to sha-34e5ef8 ([3e54d8c](https://github.com/vymalo/another-agentic-system/commit/3e54d8c8419659d87616753fdf510ac3863b970a))
* **deploy:** bump orchestrator to sha-5d967b9 ([ea46173](https://github.com/vymalo/another-agentic-system/commit/ea4617332682f593df5a2acb0b71f9b3e1a6b904))
* **deploy:** bump orchestrator to sha-6fbd910 ([fc6d29c](https://github.com/vymalo/another-agentic-system/commit/fc6d29cf5e743401b639a8b5ee5ca77a36c10f1a))
* **deploy:** bump orchestrator to sha-7ca5e05 ([40ebc2b](https://github.com/vymalo/another-agentic-system/commit/40ebc2b1bf9ea9f7dc3a8291bde28378a4ae8ad0))
* **deploy:** bump orchestrator to sha-804b67a ([2189f37](https://github.com/vymalo/another-agentic-system/commit/2189f37325966cf059669d9aeb9d0a5f0ca51d35))
* **deploy:** bump orchestrator to sha-bfafb08 ([dc4d87f](https://github.com/vymalo/another-agentic-system/commit/dc4d87f836d630e156eb8768da79a74b483dc9d4))
* **deploy:** bump orchestrator to sha-d2ca24e ([6cd8633](https://github.com/vymalo/another-agentic-system/commit/6cd863315df6f4244dd59748bf847b5b9a823957))
* **deploy:** bump web to sha-1612f61 ([1398bc6](https://github.com/vymalo/another-agentic-system/commit/1398bc65a47934c6f83d3842d7d5339cd99f802e))
* **deploy:** bump web to sha-1dc6c21 ([1bd16ad](https://github.com/vymalo/another-agentic-system/commit/1bd16ad582bb7e44daf982a425395c149ee0f85d))
* **deploy:** bump web to sha-34e5ef8 ([0ef5554](https://github.com/vymalo/another-agentic-system/commit/0ef5554850dc0edd41cec6f1a6a2afc7d690f252))
* **deploy:** bump web to sha-5d967b9 ([ab749ea](https://github.com/vymalo/another-agentic-system/commit/ab749ea27ec6ba2c275531b27a46b4e2e5d7ecbc))
* **deploy:** bump web to sha-6fbd910 ([b074d58](https://github.com/vymalo/another-agentic-system/commit/b074d58784bdb69dbd7a785a850b1f17006dab65))
* **deploy:** bump web to sha-7f7c596 ([3e0974a](https://github.com/vymalo/another-agentic-system/commit/3e0974a1e1888261089ce28be8837f905626d8f6))
* **deploy:** bump web to sha-bfafb08 ([25f598f](https://github.com/vymalo/another-agentic-system/commit/25f598fd069b8d90e86ad6939c619b5b525a9512))

## [0.4.3](https://github.com/vymalo/another-agentic-system/compare/v0.4.2...v0.4.3) (2026-10-12)


### Bug Fixes

* **release:** sign and verify Apple app bundles before delivery ([#241](https://github.com/vymalo/another-agentic-system/issues/241)) ([4c6406b](https://github.com/vymalo/another-agentic-system/commit/4c6406b0f4f964b26f548a22d970902d953b68ad))


### Chores

* **deploy:** bump web to sha-d26dc86 ([e2bc9b0](https://github.com/vymalo/another-agentic-system/commit/e2bc9b0befeebfff475dd0d6309e23cad47359ad))
* disable mobile app links in values.yaml ([0e486f8](https://github.com/vymalo/another-agentic-system/commit/0e486f853612fe96b1ab9e0b20a497822ea9ede1))

## [0.4.2](https://github.com/vymalo/another-agentic-system/compare/v0.4.1...v0.4.2) (2026-10-11)


### Bug Fixes

* **release:** verify installers and make tap upgrades idempotent ([#238](https://github.com/vymalo/another-agentic-system/issues/238)) ([d26dc86](https://github.com/vymalo/another-agentic-system/commit/d26dc86300c4c83635795d3158b2a0b2e9b14aa9))

## [0.4.1](https://github.com/vymalo/another-agentic-system/compare/v0.4.0...v0.4.1) (2026-10-11)


### Bug Fixes

* **release:** install Homebrew smoke assets from tap ([#236](https://github.com/vymalo/another-agentic-system/issues/236)) ([38de368](https://github.com/vymalo/another-agentic-system/commit/38de368c8408e14d8ce04d07ff6f683d00455d42))

## [0.4.0](https://github.com/vymalo/another-agentic-system/compare/v0.3.0...v0.4.0) (2026-10-11)


### Features

* **release:** distribute Linux arm64 desktop app ([#233](https://github.com/vymalo/another-agentic-system/issues/233)) ([89faa3c](https://github.com/vymalo/another-agentic-system/commit/89faa3cc0379472cd2ce3079f990e4584d0de0aa))


### Bug Fixes

* **release:** preserve Linux bundle asset paths ([#235](https://github.com/vymalo/another-agentic-system/issues/235)) ([40035c2](https://github.com/vymalo/another-agentic-system/commit/40035c2ba17bb9ac2c5f330f5a3cf9f2ea98ae90))


### Chores

* **deploy:** bump orchestrator to sha-8746765 ([bfc9f46](https://github.com/vymalo/another-agentic-system/commit/bfc9f46609afc9ac90efc46dec79760cc76e6071))
* **deploy:** bump web to sha-8746765 ([ae2f8e9](https://github.com/vymalo/another-agentic-system/commit/ae2f8e9005caa2f35307f800d9302416de4d21ee))

## [0.3.0](https://github.com/vymalo/another-agentic-system/compare/v0.2.0...v0.3.0) (2026-10-11)


### Features

* access tiers per message (access-tier/v1), orchestrator and web ([#228](https://github.com/vymalo/another-agentic-system/issues/228)) ([8746765](https://github.com/vymalo/another-agentic-system/commit/87467651668812af3dbadbab91f6a36771db8b0f))


### Bug Fixes

* **release:** use macOS 26 desktop runners ([#232](https://github.com/vymalo/another-agentic-system/issues/232)) ([1611fcf](https://github.com/vymalo/another-agentic-system/commit/1611fcf5a287eb1411d512a750fed4444cec08d7))

## [0.2.0](https://github.com/vymalo/another-agentic-system/compare/v0.1.0...v0.2.0) (2026-10-11)


### Features

* **desktop:** automate desktop and mobile releases ([#227](https://github.com/vymalo/another-agentic-system/issues/227)) ([3c4cc21](https://github.com/vymalo/another-agentic-system/commit/3c4cc21731438efbc1c4104fadeeed3f5c6bd308))
* **web:** A2UI components for code, structure, data and media (UI catalog version 6) ([#221](https://github.com/vymalo/another-agentic-system/issues/221)) ([cf1f4fc](https://github.com/vymalo/another-agentic-system/commit/cf1f4fc37a236a87c9470b062ad6864dc7dd8231))


### Chores

* **deploy:** bump orchestrator to sha-cf1f4fc ([ad07cd3](https://github.com/vymalo/another-agentic-system/commit/ad07cd3e9e45bce54b0a72836cbcc5e2dbbd0015))
* **deploy:** bump web to sha-3c4cc21 ([6810c9c](https://github.com/vymalo/another-agentic-system/commit/6810c9c04322c9232f4ed069981df2a3512c573b))
* **deploy:** bump web to sha-cf1f4fc ([6dca7f2](https://github.com/vymalo/another-agentic-system/commit/6dca7f22e647be6f201ea69d3271e43684756014))
