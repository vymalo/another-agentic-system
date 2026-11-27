# Changelog

## [0.9.0](https://github.com/vymalo/another-agentic-system/compare/v0.8.0...v0.9.0) (2026-11-27)


### Features

* **app:** send a person's message to their own provisioned agent ([#590](https://github.com/vymalo/another-agentic-system/issues/590)) ([794618a](https://github.com/vymalo/another-agentic-system/commit/794618a4b83e0c62861944a852b6993252ea9f5f))


### Documentation

* **adr:** renumber the later ADR of each duplicated number, 0094 and 0103 (ANR-1096) ([#603](https://github.com/vymalo/another-agentic-system/issues/603)) ([8610c68](https://github.com/vymalo/another-agentic-system/commit/8610c687bdaf3e983259408a8e11668db8716a84))
* **api:** tags, examples and unwrapped descriptions in chat-api.yaml (ANR-1083) ([#605](https://github.com/vymalo/another-agentic-system/issues/605)) ([5f56692](https://github.com/vymalo/another-agentic-system/commit/5f566925ee78a8c64699edb2a261bfdd6c600da4))
* fix stale built and not-built facts in vision, architecture, orchestrator and the api index (ANR-1125) ([#608](https://github.com/vymalo/another-agentic-system/issues/608)) ([830e4eb](https://github.com/vymalo/another-agentic-system/commit/830e4eb9cadb285ba26e25fa63139718aaeb25c0))
* GitHub slugs on the site and a docs-check rule for dead #anchors (ANR-1127) ([#610](https://github.com/vymalo/another-agentic-system/issues/610)) ([663af83](https://github.com/vymalo/another-agentic-system/commit/663af83965197d1a8a3540922049e0e8f6087b67))
* name no private repository, and a check that keeps it so ([#615](https://github.com/vymalo/another-agentic-system/issues/615)) ([63e0f92](https://github.com/vymalo/another-agentic-system/commit/63e0f92403176891fa6ae2f6519af2e0fc5f8cca))
* progressive pages with in-depth blocks and one status-icon rule (ANR-1087) ([#606](https://github.com/vymalo/another-agentic-system/issues/606)) ([926cb1d](https://github.com/vymalo/another-agentic-system/commit/926cb1ddeeb6c7af9f09c3e6b1b3423fef245462))
* **readme:** a features table with a status per row, and the ADR notes and counts it found stale ([#614](https://github.com/vymalo/another-agentic-system/issues/614)) ([e48f649](https://github.com/vymalo/another-agentic-system/commit/e48f649df483c5608f4280ebb7b288c385df9dac))
* short sidebar labels with a 40-character guard, starting with the ADRs (ANR-1092) ([#599](https://github.com/vymalo/another-agentic-system/issues/599)) ([35f8e4b](https://github.com/vymalo/another-agentic-system/commit/35f8e4baa8162e8d55c3be429595dcd87004f0a1))
* STRUCTURE.md v3, a wider sidebar, in-depth blocks and status icons (ANR-1084) ([#594](https://github.com/vymalo/another-agentic-system/issues/594)) ([f98fb19](https://github.com/vymalo/another-agentic-system/commit/f98fb196181767c86e662823bf9e515438a6857c))


### Tests

* **config:** read titled yaml fences of config.md ([#613](https://github.com/vymalo/another-agentic-system/issues/613)) ([1b075e6](https://github.com/vymalo/another-agentic-system/commit/1b075e62a77ebfe39c3022c9b7a56965a6a6d63b))
* **web:** scroll to the log's own turn in the cards and done screens (ANR-1004) ([#600](https://github.com/vymalo/another-agentic-system/issues/600)) ([df5ace5](https://github.com/vymalo/another-agentic-system/commit/df5ace5ba710405df01e0ab54c06fe532584151a))
* **web:** wait for a mention pick before typing on, in the mentions-warning screen ([#595](https://github.com/vymalo/another-agentic-system/issues/595)) ([2720425](https://github.com/vymalo/another-agentic-system/commit/2720425d5fd354392901f527fdc0cc5c87120f0b))
* **web:** wait for the focus and the error words of a refused audience (ANR-1138) ([#611](https://github.com/vymalo/another-agentic-system/issues/611)) ([dbafabb](https://github.com/vymalo/another-agentic-system/commit/dbafabb2060107028e72a9ef398d94bc27fc3ae7))


### Build System

* **deploy:** Helm 4 only and Kubernetes 1.37 as the floor (ADR 0103) ([#591](https://github.com/vymalo/another-agentic-system/issues/591)) ([0cc6b29](https://github.com/vymalo/another-agentic-system/commit/0cc6b29ce36c12f7ecac372efee32b086dccc4b7))
* **docs:** draw diagrams with vitepress-mermaid-renderer in the Material 3 colours (ANR-1091) ([#601](https://github.com/vymalo/another-agentic-system/issues/601)) ([030e2e0](https://github.com/vymalo/another-agentic-system/commit/030e2e0c38933c67ec0fb53528b47b858dc04ab3))
* **docs:** group icons on titled fences and code groups (ANR-1089) ([#602](https://github.com/vymalo/another-agentic-system/issues/602)) ([8ea1cdb](https://github.com/vymalo/another-agentic-system/commit/8ea1cdb320e5cdd0c2939a9f63aeb2098c4db51b))
* **docs:** mermaid 12.3.0 in the docs checker, the docs site and the web ([#612](https://github.com/vymalo/another-agentic-system/issues/612)) ([a6fb6d7](https://github.com/vymalo/another-agentic-system/commit/a6fb6d7946be65611564b0c5cdf3a85b315a6995))
* **docs:** publish llms.txt, llms-full.txt and a .md per page (ANR-1088) ([#598](https://github.com/vymalo/another-agentic-system/issues/598)) ([4a301c5](https://github.com/vymalo/another-agentic-system/commit/4a301c5089587ee9961e2e60aa416c3e19fd1c00))


### Continuous Integration

* **release:** a tag publishes the orchestrator and web images and the chart at X.Y.Z, signed (ADR 0106) ([#616](https://github.com/vymalo/another-agentic-system/issues/616)) ([98c20b6](https://github.com/vymalo/another-agentic-system/commit/98c20b6af15e5ca4d1c198d080cd16cf1cbabf12))
* **web:** update-screens commits only the stills beyond the bound (ANR-1128) ([#607](https://github.com/vymalo/another-agentic-system/issues/607)) ([a7ce933](https://github.com/vymalo/another-agentic-system/commit/a7ce9334e19b866ffe98829c6afc4b1443dbcdff))


### Chores

* **deploy:** bump orchestrator to sha-63e0f92 ([86845b6](https://github.com/vymalo/another-agentic-system/commit/86845b62a30cf2005fde7650657c582eb7533d7c))
* **deploy:** bump orchestrator to sha-794618a ([e1dd01b](https://github.com/vymalo/another-agentic-system/commit/e1dd01b264608d7ace2061720fc747652cb5c78b))
* **deploy:** bump web to sha-2720425 ([01598f4](https://github.com/vymalo/another-agentic-system/commit/01598f4ea27ddaeb987465fad93606e666717567))
* **deploy:** bump web to sha-3ec5059 ([a84b5ee](https://github.com/vymalo/another-agentic-system/commit/a84b5ee09b2847a7e607703ea8842fa719e7dce9))
* **deploy:** bump web to sha-663af83 ([5864ef5](https://github.com/vymalo/another-agentic-system/commit/5864ef5e5b7ba3454592049c95abcc6622a0ba87))
* **deploy:** bump web to sha-a6fb6d7 ([a948ef4](https://github.com/vymalo/another-agentic-system/commit/a948ef444925fcba7b5c7f2db64fac76ee3d9da3))
* **deploy:** bump web to sha-dbafabb ([76ff3e0](https://github.com/vymalo/another-agentic-system/commit/76ff3e034bdc682b4ecf3990672ec49887449ee9))
* **deploy:** bump web to sha-df5ace5 ([179aefd](https://github.com/vymalo/another-agentic-system/commit/179aefd4cc0330b385aea382a0154aa41ff5eb34))
* **web:** re-baseline the screens from CI's own image ([#597](https://github.com/vymalo/another-agentic-system/issues/597)) ([6743129](https://github.com/vymalo/another-agentic-system/commit/6743129cd7eb86c661300c005340eb85f57713e1))
* **web:** re-baseline the screens from CI's own image ([#604](https://github.com/vymalo/another-agentic-system/issues/604)) ([3ec5059](https://github.com/vymalo/another-agentic-system/commit/3ec5059fd3e67aeb810fd2ce1465004c7137272f))

## [0.8.0](https://github.com/vymalo/another-agentic-system/compare/v0.7.1...v0.8.0) (2026-11-27)


### ⚠ BREAKING CHANGES

* **api:** a store outside this repository must implement PromptStore.

### Features

* a person drives a child session: start, send, stop and close (S3 of [#332](https://github.com/vymalo/another-agentic-system/issues/332)) ([#395](https://github.com/vymalo/another-agentic-system/issues/395)) ([eb0bed7](https://github.com/vymalo/another-agentic-system/commit/eb0bed7057f4bd21c346709e7d5de1f32981edd2))
* **a2a:** parse and activate child-sessions/v1 on the agent side (S2c) ([#352](https://github.com/vymalo/another-agentic-system/issues/352)) ([469e1ee](https://github.com/vymalo/another-agentic-system/commit/469e1eed42eca23d6d0fa5942e8841a203fdfea2))
* **agent-adam:** LocalAgents::sqlite runs the local agents on the SQLite store in the embedded build ([#558](https://github.com/vymalo/another-agentic-system/issues/558)) ([2b2f08f](https://github.com/vymalo/another-agentic-system/commit/2b2f08f9ccc37380407d9d7f84e80cb7a29562f2))
* **agui:** a child session is a subagent and a card in the parent's stream (S2e) ([#356](https://github.com/vymalo/another-agentic-system/issues/356)) ([fe055cb](https://github.com/vymalo/another-agentic-system/commit/fe055cba9e5308e0546532d9c0930e91276ddab4))
* **agui:** a stream ends after a minute and reopens; a message keeps the id its client made ([#427](https://github.com/vymalo/another-agentic-system/issues/427)) ([98d6d61](https://github.com/vymalo/another-agentic-system/commit/98d6d61e29f80efbb53aa4da81608d9a9a89a689))
* **agui:** a trusted request reaches the client as an activity, not a surface ([#433](https://github.com/vymalo/another-agentic-system/issues/433)) ([05c8acc](https://github.com/vymalo/another-agentic-system/commit/05c8acc976612de2481e8e30fb1fd90d3cda7226))
* **api:** a person searches their own threads (the API, the index, the contract and the mock) ([#322](https://github.com/vymalo/another-agentic-system/issues/322)) ([00a4a30](https://github.com/vymalo/another-agentic-system/commit/00a4a306023da8f0fa14ca4f21387dbf1701f710))
* **api:** a person's prompt library, the API half (ADR 0070) ([#324](https://github.com/vymalo/another-agentic-system/issues/324)) ([bb4d722](https://github.com/vymalo/another-agentic-system/commit/bb4d72275353d9ab166eea924cc9b9e6ff846fd0))
* **api:** a saved typed key returns its last four characters ([#494](https://github.com/vymalo/another-agentic-system/issues/494)) ([40b4467](https://github.com/vymalo/another-agentic-system/commit/40b4467f183051796caa25317ac2a7d974c88486))
* **api:** attachments upload with a hard 10 MiB ceiling ([#492](https://github.com/vymalo/another-agentic-system/issues/492)) ([474aa20](https://github.com/vymalo/another-agentic-system/commit/474aa20617f84764f613d371c02437641b0fc55d))
* **api:** CBOR as a second media type of the chat API's JSON bodies ([#569](https://github.com/vymalo/another-agentic-system/issues/569)) ([be42527](https://github.com/vymalo/another-agentic-system/commit/be42527fb26be9754047fd698bd9a2f968d2c577))
* **api:** database pool gauges on /metrics and the run-pod quota query ([7a5044d](https://github.com/vymalo/another-agentic-system/commit/7a5044df3c80cfe55ac7dda7bdf53ce238d593c3))
* **api:** refuse an application below the minimum client version ([#500](https://github.com/vymalo/another-agentic-system/issues/500)) ([ede3b13](https://github.com/vymalo/another-agentic-system/commit/ede3b134ee539bde26c9853eb9ef5154e7c66db4))
* **api:** team presets API, /api/teams and teams in /api/config ([#511](https://github.com/vymalo/another-agentic-system/issues/511)) ([ec03b58](https://github.com/vymalo/another-agentic-system/commit/ec03b58597d003fa311d592225e3ca5183bf8797))
* **api:** the agent list carries the card's model and reasoning listings ([#459](https://github.com/vymalo/another-agentic-system/issues/459)) ([e2891dc](https://github.com/vymalo/another-agentic-system/commit/e2891dc16803673fbc998eda5c80c0d816802f36))
* **app:** a title's lease is the thread's, through the injector (ANR-670) ([#538](https://github.com/vymalo/another-agentic-system/issues/538)) ([58c58d9](https://github.com/vymalo/another-agentic-system/commit/58c58d97cffa12c050ff7e08c38953556f9d4313))
* **app:** bind a run when its task id is seen, unbind it at every end (ADR 0084) ([#462](https://github.com/vymalo/another-agentic-system/issues/462)) ([6a64770](https://github.com/vymalo/another-agentic-system/commit/6a64770d54dad2490339d2a7bf911e4d958ff604))
* **app:** follow child sessions and make their threads (S2d) ([#355](https://github.com/vymalo/another-agentic-system/issues/355)) ([a900bc3](https://github.com/vymalo/another-agentic-system/commit/a900bc334f70403f8570c0f39cb2541c4302a398))
* **app:** make or resume a person's own agent before a message is accepted ([e4080c7](https://github.com/vymalo/another-agentic-system/commit/e4080c7016a8bd3e5e4b126811a1636182371aae))
* **app:** refuse a message over the role's tokensPerPeriod with 403 allowance_spent before any agent is called ([#553](https://github.com/vymalo/another-agentic-system/issues/553)) ([0b85cb3](https://github.com/vymalo/another-agentic-system/commit/0b85cb391798c359b10bea85aed4f0c1d85ee415))
* **broker:** an internal route from run to account id, for the meter identity alone (ANR-663) ([#510](https://github.com/vymalo/another-agentic-system/issues/510)) ([f4f8641](https://github.com/vymalo/another-agentic-system/commit/f4f8641f85b15c31adc862e5ea55ecf58ebe11ef))
* **broker:** the run lease carries the person's ledger account id ([#505](https://github.com/vymalo/another-agentic-system/issues/505)) ([13060ae](https://github.com/vymalo/another-agentic-system/commit/13060ae7bd4ae8b98d35696f2eb4807adae5154a))
* **chart:** a rate limit per client address at the Ingress, off by default ([#307](https://github.com/vymalo/another-agentic-system/issues/307)) ([a34c14c](https://github.com/vymalo/another-agentic-system/commit/a34c14c4756da103c3bebddc626bf34a2881dadf))
* **chart:** three database instances, resources and a transaction pooler (sys[#290](https://github.com/vymalo/another-agentic-system/issues/290), PR 2) ([#310](https://github.com/vymalo/another-agentic-system/issues/310)) ([26d9c17](https://github.com/vymalo/another-agentic-system/commit/26d9c17494696024bfeb2dd4482e314a4e36046c))
* **chart:** web search over our own SearXNG by default, Brave by choice ([#580](https://github.com/vymalo/another-agentic-system/issues/580)) ([886a1c2](https://github.com/vymalo/another-agentic-system/commit/886a1c2013e9b18b3fdd28e49bdd78a1e2015be5))
* **chat:** language and trusted-string rules in the chat folder ([#446](https://github.com/vymalo/another-agentic-system/issues/446)) ([4e2e161](https://github.com/vymalo/another-agentic-system/commit/4e2e16115b4bd099d170b38657e23fc045aec0ec))
* **chat:** locale/v1 on the chat's card, the language rules, and the adam pin ([#451](https://github.com/vymalo/another-agentic-system/issues/451)) ([033b071](https://github.com/vymalo/another-agentic-system/commit/033b071100b7e87c15e21ac92a6e88c54e766ea2))
* **child-sessions:** child-sessions/v1 contract, ADR 0072, the card read into an offer (S1) ([#347](https://github.com/vymalo/another-agentic-system/issues/347)) ([2654109](https://github.com/vymalo/another-agentic-system/commit/26541095e4d2560f373625800bb7b81fb515d249))
* **config:** auth.signingClients and the presence check of a signing client's envelope ([#535](https://github.com/vymalo/another-agentic-system/issues/535)) ([1320d8c](https://github.com/vymalo/another-agentic-system/commit/1320d8c3939311909af95d46f6312423ad14f6cf))
* **config:** per-role limits section with its schema, runs_at_once defaulting to 4 ([#543](https://github.com/vymalo/another-agentic-system/issues/543)) ([a079e92](https://github.com/vymalo/another-agentic-system/commit/a079e9244b25f9293f9bd94004e79664b7aad3fa))
* **core:** child sessions in the core, the parent's three events and the observed round (S2a) ([#349](https://github.com/vymalo/another-agentic-system/issues/349)) ([af1a5f1](https://github.com/vymalo/another-agentic-system/commit/af1a5f1adb8636ea9d1998fba65b612ecd0699d0))
* **core:** hand_off, the trailing [@member](https://github.com/member) line and fan-in results (ANR-169) ([#498](https://github.com/vymalo/another-agentic-system/issues/498)) ([0ccff4f](https://github.com/vymalo/another-agentic-system/commit/0ccff4ff90b0a6416b91608f17b913fe978b0b87))
* **core:** notification intents folded from the log ([#490](https://github.com/vymalo/another-agentic-system/issues/490)) ([67455b7](https://github.com/vymalo/another-agentic-system/commit/67455b74c0356a8758570a3de82c6afcf7f302c4))
* **core:** the approval gate of a team hand-off (ANR-180) ([#537](https://github.com/vymalo/another-agentic-system/issues/537)) ([d44f5b9](https://github.com/vymalo/another-agentic-system/commit/d44f5b9632fcaae3bf5829e0cea5e381d7bcaa4f))
* **core:** the memory types and rules, and the web mock's, both pass the memory-v1 vectors ([#517](https://github.com/vymalo/another-agentic-system/issues/517)) ([fc9b293](https://github.com/vymalo/another-agentic-system/commit/fc9b293aa42d8bea8238644e0235184b6eca0925))
* **core:** the pure mission core, its events, states and transitions (ADR 0095 slice 3a) ([#539](https://github.com/vymalo/another-agentic-system/issues/539)) ([47bda3a](https://github.com/vymalo/another-agentic-system/commit/47bda3a623a06d06c848986c551aee8f52293b4f))
* **core:** the team ledger, its routing, its cap and its fan-in (ANR-163) ([#491](https://github.com/vymalo/another-agentic-system/issues/491)) ([2c15aa7](https://github.com/vymalo/another-agentic-system/commit/2c15aa77a5e1e249b970c407f107649fc64be637))
* **credential-broker:** the internal run routes (ADR 0084 decisions 1, 2, 5, 6) ([#457](https://github.com/vymalo/another-agentic-system/issues/457)) ([f6aee00](https://github.com/vymalo/another-agentic-system/commit/f6aee0056528a64759b51d4b70cd90fe33244d03))
* **credential-broker:** the RunBindings port and the orchestrator's client of it ([#458](https://github.com/vymalo/another-agentic-system/issues/458)) ([1c70dd7](https://github.com/vymalo/another-agentic-system/commit/1c70dd723ddb7af465c9bf12f645337877c14841))
* **deploy:** inject the run lease's key and account id ([#514](https://github.com/vymalo/another-agentic-system/issues/514)) ([64f97e7](https://github.com/vymalo/another-agentic-system/commit/64f97e7d4a7ce25b87b79f3ab4e6d824b26d08cb))
* **deploy:** the chart can prepare the run routes' internal listener ([#460](https://github.com/vymalo/another-agentic-system/issues/460)) ([81c8b5a](https://github.com/vymalo/another-agentic-system/commit/81c8b5a278afe3bb3431787b0d6babbd2c71c193))
* **deploy:** the chart names the platform's agent registry, url and token reference (ANR-685) ([#499](https://github.com/vymalo/another-agentic-system/issues/499)) ([48ce1a2](https://github.com/vymalo/another-agentic-system/commit/48ce1a23aec8341a7085c8e74951bc43d0913d7b))
* **deploy:** the database cluster follows the release's node selector and tolerations ([#529](https://github.com/vymalo/another-agentic-system/issues/529)) ([3adf3b7](https://github.com/vymalo/another-agentic-system/commit/3adf3b742bd543e476531e29ac729700f2f3a49d))
* **deploy:** the model-credential injector, off by default (S9) ([#488](https://github.com/vymalo/another-agentic-system/issues/488)) ([4c8befd](https://github.com/vymalo/another-agentic-system/commit/4c8befd5489a95129a2ccef528837b53d21b0f2f))
* **deploy:** the orchestrator chart behind the BFF only — web and edge off, the BFF as the one peer, sub claim ([#497](https://github.com/vymalo/another-agentic-system/issues/497)) ([05ef902](https://github.com/vymalo/another-agentic-system/commit/05ef902206ad38ccc9acc1ca0eba7829a1a14035))
* **dev:** the S9 spike — Agent Router, a mock model and the ext-auth header proof ([#437](https://github.com/vymalo/another-agentic-system/issues/437)) ([61776db](https://github.com/vymalo/another-agentic-system/commit/61776db8efefe4f91a2d97aa1460e904bfdbeafd))
* **dispatcher:** a fair share of the dispatcher for each person (ADR 0066) ([#306](https://github.com/vymalo/another-agentic-system/issues/306)) ([f3ea64f](https://github.com/vymalo/another-agentic-system/commit/f3ea64ff2daf13fefa19990b9313da4380351c7b))
* **dispatcher:** bind the run's model endpoint from the live card's pick (ANR-624) ([#470](https://github.com/vymalo/another-agentic-system/issues/470)) ([52d2a74](https://github.com/vymalo/another-agentic-system/commit/52d2a74099dfa58b4a65fed1e664049588199e54))
* **embed:** hand the application's model, file store and user-tool ports to the embedded root ([#525](https://github.com/vymalo/another-agentic-system/issues/525)) ([613cc72](https://github.com/vymalo/another-agentic-system/commit/613cc7272eeb2b51df7d54dc85241f26f85b9854))
* **embed:** replay from a seq, a client message id, the limit event and the version gate (ANR-757) ([#546](https://github.com/vymalo/another-agentic-system/issues/546)) ([e9b11c5](https://github.com/vymalo/another-agentic-system/commit/e9b11c5911c816915a41075f30ea2912ecccf94f))
* **embed:** the library composition root and a thread example (E3 part 1) ([#467](https://github.com/vymalo/another-agentic-system/issues/467)) ([c3438b3](https://github.com/vymalo/another-agentic-system/commit/c3438b3dc6e7c9e585cc3fa5467e95a5c820da80))
* **injector:** read the person's balance at every model call and answer 402 when it is spent ([#533](https://github.com/vymalo/another-agentic-system/issues/533)) ([d8eb54f](https://github.com/vymalo/another-agentic-system/commit/d8eb54f3affb5b8d90682b3b65a41962d6b2a06b))
* **limits:** refuse a message over the role's messages per minute, naming the limit ([#557](https://github.com/vymalo/another-agentic-system/issues/557)) ([a819d16](https://github.com/vymalo/another-agentic-system/commit/a819d16f0676381fdd304bf61e564c2aa5e9a65f))
* **locale:** the interface language travels with a message, as a hint ([#443](https://github.com/vymalo/another-agentic-system/issues/443)) ([9febb68](https://github.com/vymalo/another-agentic-system/commit/9febb6895e77008dbb10d63a11a04952ab6dfaee))
* **mcp:** add initial MCP server configuration ([f925db4](https://github.com/vymalo/another-agentic-system/commit/f925db4b0e365642823b4841c2f9f0d5a5be612e))
* **mcp:** serve a person's prompt library as MCP prompts (ANR-189) ([#487](https://github.com/vymalo/another-agentic-system/issues/487)) ([84ec815](https://github.com/vymalo/another-agentic-system/commit/84ec81501abd0d236231625495b5ce1eefe558f3))
* **metrics:** dispatcher slots in use and open SSE subscribers, with no label ([#509](https://github.com/vymalo/another-agentic-system/issues/509)) ([58d76cb](https://github.com/vymalo/another-agentic-system/commit/58d76cb026ea0ad6f7d0b4a8ea2768fdb37b1960))
* **model-pick:** the orchestrator carries a per-message model and reasoning pick and stores it ([#454](https://github.com/vymalo/another-agentic-system/issues/454)) ([5d1b830](https://github.com/vymalo/another-agentic-system/commit/5d1b83020decebb9acc3a6e0fc42fa61d1444bad))
* **opencode:** add MCP server configuration for corntchap-and ([9f837a6](https://github.com/vymalo/another-agentic-system/commit/9f837a6dbe88b62f7516c85162143bf95ad4ef62))
* **orch:** A2UI v1.0 only, a hard cutover (S1 of [#380](https://github.com/vymalo/another-agentic-system/issues/380), ADR 0075) ([#410](https://github.com/vymalo/another-agentic-system/issues/410)) ([ebcee35](https://github.com/vymalo/another-agentic-system/commit/ebcee3509985ba9966d1dc5461c3f61898f9162e))
* **orchestrator:** AgentProvisioner port and a Platform API adapter ([#547](https://github.com/vymalo/another-agentic-system/issues/547)) ([bff02c8](https://github.com/vymalo/another-agentic-system/commit/bff02c85a267a1a4d6e57cc24775767b4624dca6))
* **orchestrator:** database.directUrl, so a transaction pooler can carry the rest (sys[#290](https://github.com/vymalo/another-agentic-system/issues/290), PR 1) ([#309](https://github.com/vymalo/another-agentic-system/issues/309)) ([c777d29](https://github.com/vymalo/another-agentic-system/commit/c777d29bfb23f92a208e44180bd530efd0e178fa))
* **orchestrator:** erase idle threads after retention.threads.days, default 90 ([#549](https://github.com/vymalo/another-agentic-system/issues/549)) ([a1a5312](https://github.com/vymalo/another-agentic-system/commit/a1a53127c15b845a7f9b93b43c41fccd685d1414))
* **orchestrator:** serve the run routes on an internal listener ([#463](https://github.com/vymalo/another-agentic-system/issues/463)) ([b35ecc3](https://github.com/vymalo/another-agentic-system/commit/b35ecc303e329147a0273f5efc1f4dadfe0b1e14))
* **orch:** list an agent's own commands from its card (ADR 0079) ([#394](https://github.com/vymalo/another-agentic-system/issues/394)) ([4d7c915](https://github.com/vymalo/another-agentic-system/commit/4d7c915f2d8a10af005fa299b15ee40ad6ca88f7))
* **pg:** a migration runner and a connection that continue what sqlx did ([#425](https://github.com/vymalo/another-agentic-system/issues/425)) ([7fcad12](https://github.com/vymalo/another-agentic-system/commit/7fcad125745961e603a1e8077978092e9c8f22f7))
* **ports:** AccountErase port, conformance testkit and in-memory adapter (ADR 0098, slice E2) ([#552](https://github.com/vymalo/another-agentic-system/issues/552)) ([9750523](https://github.com/vymalo/another-agentic-system/commit/9750523910e2d93ab706fd8d27517c4fdc9709e1))
* **ports:** admit_commit admits a message under a per-person lock, runs_at_once ([#555](https://github.com/vymalo/another-agentic-system/issues/555)) ([4268e9d](https://github.com/vymalo/another-agentic-system/commit/4268e9d587a789a3efb7af03113e0d7dfbf72446))
* **ports:** an entitlement source port with a memory fake and a conformance testkit ([#550](https://github.com/vymalo/another-agentic-system/issues/550)) ([baf90ff](https://github.com/vymalo/another-agentic-system/commit/baf90ff085ff00ae377f81b735b4a0ef02c0e35a))
* **ports:** the PromptSigner port, the prompt statement builder and a static test signer (ANR-715) ([#530](https://github.com/vymalo/another-agentic-system/issues/530)) ([ba532a6](https://github.com/vymalo/another-agentic-system/commit/ba532a6c667ac432e2ff2b19505913f0c5f35e7d))
* **ports:** the run lease — the broker's first part (B1, ADR 0084) ([#441](https://github.com/vymalo/another-agentic-system/issues/441)) ([4b6c5b7](https://github.com/vymalo/another-agentic-system/commit/4b6c5b78b98a4ef1842028c71fd8d7fb666df417))
* **ports:** the TeamPresetStore port, its conformance testkit and the Postgres adapter (ANR-166) ([#493](https://github.com/vymalo/another-agentic-system/issues/493)) ([b7cbece](https://github.com/vymalo/another-agentic-system/commit/b7cbecee0629bc1fbe207eb28136b5f231197a7f))
* **provisioner:** the provisioner config, the PlatformCredentials port and a dev adapter ([#563](https://github.com/vymalo/another-agentic-system/issues/563)) ([b7f3e5e](https://github.com/vymalo/another-agentic-system/commit/b7f3e5e4689c0f5062ff97cc4cabacf589c730d3))
* **secrets:** a versioned sealed store for vault:&lt;name&gt;#v&lt;n&gt; (ANR-642) ([#486](https://github.com/vymalo/another-agentic-system/issues/486)) ([d88d8b7](https://github.com/vymalo/another-agentic-system/commit/d88d8b70a5906e7fa23f396be0cb1811bbc8efd0))
* **secrets:** an OpenBao adapter behind the secret store port ([#526](https://github.com/vymalo/another-agentic-system/issues/526)) ([e894aea](https://github.com/vymalo/another-agentic-system/commit/e894aea05d2b833e13779efbb2a2db8f5eb05cb7))
* **secrets:** choose the OpenBao secret store in the configuration and the chart (ANR-709) ([#527](https://github.com/vymalo/another-agentic-system/issues/527)) ([1f553fc](https://github.com/vymalo/another-agentic-system/commit/1f553fc14772cefb336dc6360bab3b9be4295987))
* **store-sqlite:** a ThreadStore on SQLite for a thread whose home is an application ([#382](https://github.com/vymalo/another-agentic-system/issues/382) E2) ([#439](https://github.com/vymalo/another-agentic-system/issues/439)) ([e95eafc](https://github.com/vymalo/another-agentic-system/commit/e95eafcdfb7d8e151e155eb6410e5619af89673e))
* **store-sqlite:** PendingAuthorizations, its migration and the conformance (ANR-125) ([#465](https://github.com/vymalo/another-agentic-system/issues/465)) ([324ed15](https://github.com/vymalo/another-agentic-system/commit/324ed157655c38d509dd0458df231b0bc42f6768))
* **store-sqlite:** PromptStore and an in-process Wakeup (ANR-123, ANR-126) ([#448](https://github.com/vymalo/another-agentic-system/issues/448)) ([edf180b](https://github.com/vymalo/another-agentic-system/commit/edf180b697890732ca15a4d3d10ab99c7016dac3))
* **store-sqlite:** the usage ledger and the search index, on SQLite ([#444](https://github.com/vymalo/another-agentic-system/issues/444)) ([714da1e](https://github.com/vymalo/another-agentic-system/commit/714da1e43f139c0a8c47a4173d124ffb426c2055))
* **store-sqlite:** UserToolServerStore, the conformance and the migration (ANR-124) ([#461](https://github.com/vymalo/another-agentic-system/issues/461)) ([797fc7f](https://github.com/vymalo/another-agentic-system/commit/797fc7fa4c6dd370df952fe3768fd0164631555c))
* **store:** child threads, migration 0027 and the Observe outbox payload (S2b) ([#351](https://github.com/vymalo/another-agentic-system/issues/351)) ([6105fe2](https://github.com/vymalo/another-agentic-system/commit/6105fe29036a8953ebcbd7bba158230707a63408))
* **tools-mcp:** call 2026-07-28 tool servers with requests signed for the thread's owner ([#311](https://github.com/vymalo/another-agentic-system/issues/311)) ([b8b112f](https://github.com/vymalo/another-agentic-system/commit/b8b112f9c3bae68152da9d3777fcb4ab3ef1d4a4))
* **usage:** a usage ledger per person, written in the commit of the event (ADR 0065, PR 1) ([#303](https://github.com/vymalo/another-agentic-system/issues/303)) ([0d50c42](https://github.com/vymalo/another-agentic-system/commit/0d50c4275edc86ee28b1f4c9bb4cf9d2cd730cfa))
* **usage:** price a person's month and answer GET /api/me/usage, with a retention sweep (ADR 0065, PR 2) ([#305](https://github.com/vymalo/another-agentic-system/issues/305)) ([b97de39](https://github.com/vymalo/another-agentic-system/commit/b97de39811ef9a93dffb8ebd467cb0405ec88308))
* **usage:** read and log the sandbox time a task reports in usage/v1 ([#302](https://github.com/vymalo/another-agentic-system/issues/302)) ([6fc8060](https://github.com/vymalo/another-agentic-system/commit/6fc80609f246b5c68f20950d54b4bfc8c492c219))
* **web:** a My tool servers page for the MCP servers a person adds, with a client recipe in the contract (ADR 0046) ([#316](https://github.com/vymalo/another-agentic-system/issues/316)) ([716c260](https://github.com/vymalo/another-agentic-system/commit/716c260afa7047b043c173ddf2d810c9ebed6368))
* **web:** a theme the person chooses, system, light or dark, set before the first paint ([#318](https://github.com/vymalo/another-agentic-system/issues/318)) ([b79132a](https://github.com/vymalo/another-agentic-system/commit/b79132aa9a4507e75a0c81791e16a7227a09f55d))
* **web:** a usage page over GET /api/me/usage (ADR 0065) ([#312](https://github.com/vymalo/another-agentic-system/issues/312)) ([f63fde1](https://github.com/vymalo/another-agentic-system/commit/f63fde1492d879f1af00c5d4c4f9d15e432df518))
* **web:** highlighted code and display math in answers, with a message-text contract for any client ([#314](https://github.com/vymalo/another-agentic-system/issues/314)) ([632384f](https://github.com/vymalo/another-agentic-system/commit/632384fb7f367d36d84eb4196951e638a0699503))
* **web:** one slash menu with the agent's commands and the person's prompts (ANR-187, wip) ([#484](https://github.com/vymalo/another-agentic-system/issues/484)) ([729dbdb](https://github.com/vymalo/another-agentic-system/commit/729dbdb81dbbfaef026fba2478afd363206592a9))
* **web:** quote a message, copy it as Markdown, export a thread as Markdown ([#319](https://github.com/vymalo/another-agentic-system/issues/319)) ([2273c9b](https://github.com/vymalo/another-agentic-system/commit/2273c9be215b797d2a939a04a0c96d8680722add))
* **web:** read an answer aloud, and the keyboard help (ADR 0078) ([#388](https://github.com/vymalo/another-agentic-system/issues/388)) ([3875e07](https://github.com/vymalo/another-agentic-system/commit/3875e0709aba10ceb0f85f774804fca7ccaaa2c7))
* **web:** regenerate the last answer, with a client-neutral recipe in the API ([#313](https://github.com/vymalo/another-agentic-system/issues/313)) ([d5e48f5](https://github.com/vymalo/another-agentic-system/commit/d5e48f590f1519af5a48ab3b79d4e0806cc7148f))
* **web:** search my chats from a box under New chat ([#323](https://github.com/vymalo/another-agentic-system/issues/323)) ([37600d7](https://github.com/vymalo/another-agentic-system/commit/37600d76a867cf09c08195f51e443fc825c31676))
* **web:** state the application version and show an update screen on client_update_required ([#534](https://github.com/vymalo/another-agentic-system/issues/534)) ([49eef15](https://github.com/vymalo/another-agentic-system/commit/49eef15eb9795db11cfe400178316c4d6ca2a374))
* **web:** subagents in the web: cards, the Subagents tab, the child view (S4 of [#332](https://github.com/vymalo/another-agentic-system/issues/332)) ([#411](https://github.com/vymalo/another-agentic-system/issues/411)) ([71d1fe2](https://github.com/vymalo/another-agentic-system/commit/71d1fe2faf0a6ba841621abc1e10f6aa16fa5d18))
* **web:** the / menu of an agent's own commands (slice 11) ([#398](https://github.com/vymalo/another-agentic-system/issues/398)) ([89762da](https://github.com/vymalo/another-agentic-system/commit/89762da4c7a680870545cd066f38192c41562455))
* **web:** the / menu of prompts and the form that fills one ([#350](https://github.com/vymalo/another-agentic-system/issues/350)) ([9c626db](https://github.com/vymalo/another-agentic-system/commit/9c626dbe1e8c73c717d4886deb73003cdcc11272))
* **web:** the client draws a trusted component and answers with a receipt ([#438](https://github.com/vymalo/another-agentic-system/issues/438)) ([2522fcd](https://github.com/vymalo/another-agentic-system/commit/2522fcde457a73b7fa9e388c2e0251e7b20c3c95))
* **web:** the model and reasoning chips, sent with each message, and the dev/ pick e2e ([#466](https://github.com/vymalo/another-agentic-system/issues/466)) ([6e75e47](https://github.com/vymalo/another-agentic-system/commit/6e75e47397b1d86819a704983e43ad221c16c2e1))
* **web:** the page "My prompts" (ADR 0070, slice P1) ([#325](https://github.com/vymalo/another-agentic-system/issues/325)) ([f367469](https://github.com/vymalo/another-agentic-system/commit/f367469b8ab5b1919c95aafc92ed48e2be4abd53))
* **web:** the pick listing, read and sent as the contract has it ([#464](https://github.com/vymalo/another-agentic-system/issues/464)) ([92613d5](https://github.com/vymalo/another-agentic-system/commit/92613d5f8197521d5a5a77d1f507dcf3c9178365))
* **web:** UI catalog version 7 with confirm, number, scale, date and rank questions, and a contract with conformance vectors ([#317](https://github.com/vymalo/another-agentic-system/issues/317)) ([5ed8950](https://github.com/vymalo/another-agentic-system/commit/5ed8950f09383220cc1bca26c7798b0c7ea135c7))
* **web:** UI catalog version 8: a question that shows what it asks about, and a plan ([#320](https://github.com/vymalo/another-agentic-system/issues/320)) ([d0a732f](https://github.com/vymalo/another-agentic-system/commit/d0a732f21d93485835d7287e9f09a25d2b4fb338))
* **window:** the Window port, its URL guard, a conformance testkit and a fake adapter ([#522](https://github.com/vymalo/another-agentic-system/issues/522)) ([7d18087](https://github.com/vymalo/another-agentic-system/commit/7d180879869c388b770683f53e8069fbd60927e5))


### Bug Fixes

* **agui:** a surface sent again keeps only its live copy for replay; an asked agent's forms catalog has no show ([#321](https://github.com/vymalo/another-agentic-system/issues/321)) ([5cab8fc](https://github.com/vymalo/another-agentic-system/commit/5cab8fc8216921d0567e1d2bd65a9bdec8d48680))
* **broker:** a run's lease answers the orchestrator and the injector, never an agent ([#481](https://github.com/vymalo/another-agentic-system/issues/481)) ([7a509cb](https://github.com/vymalo/another-agentic-system/commit/7a509cb2c3089ed10616f7c7e3736e0e2982979a))
* **chart:** the cert-manager pin of backup-e2e names the digest of v1.21.3 ([#582](https://github.com/vymalo/another-agentic-system/issues/582)) ([79337f4](https://github.com/vymalo/another-agentic-system/commit/79337f4b99d4001a54661dea12eaac9706e74ca7))
* **ci:** fail the pooler job where PgBouncer is not ready ([#416](https://github.com/vymalo/another-agentic-system/issues/416)) ([d0b2dec](https://github.com/vymalo/another-agentic-system/commit/d0b2decf38aef10faf4333c3c08a24aa3c3cceb8))
* **ci:** start the Postgres 19 containers and read the JSON entry of the contract ([#575](https://github.com/vymalo/another-agentic-system/issues/575)) ([1fd60d0](https://github.com/vymalo/another-agentic-system/commit/1fd60d08954b804c35e96916e7e35c89a5d9d994))
* **core:** refuse the four catalog rules ui-catalog/v2 names ([#431](https://github.com/vymalo/another-agentic-system/issues/431)) ([#442](https://github.com/vymalo/another-agentic-system/issues/442)) ([35d1001](https://github.com/vymalo/another-agentic-system/commit/35d1001d36ec29f8b19bd7a7128ccde2f071b271))
* **deps:** take the advisories of tokio, rustls-webpki, webpki-roots, ctutils and zip ([#300](https://github.com/vymalo/another-agentic-system/issues/300)) ([6224b9c](https://github.com/vymalo/another-agentic-system/commit/6224b9c82f56c28ef1cdd21f2220f454973c6d70))
* **dev:** no compose project name, so each worktree has its own stack ([#489](https://github.com/vymalo/another-agentic-system/issues/489)) ([df2d993](https://github.com/vymalo/another-agentic-system/commit/df2d9939cf55594ae63da4d12aa8c62012536a0e))
* **dev:** one compose project per checkout, no fixed name ([#449](https://github.com/vymalo/another-agentic-system/issues/449)) ([0c770d8](https://github.com/vymalo/another-agentic-system/commit/0c770d8220757ba76d811371af2ab37d8f910545))
* **mcp:** correct server name from 'corntchap-anr' to 'corntchap-anr-sys' ([e19279a](https://github.com/vymalo/another-agentic-system/commit/e19279a868636aecb3ed6a524e3f9b689dd98dae))
* **orchestrator:** arm SIGTERM before the address is bound, and make three test suites deterministic ([#304](https://github.com/vymalo/another-agentic-system/issues/304)) ([6739a06](https://github.com/vymalo/another-agentic-system/commit/6739a06ef286474024e38d049ca016e50ede8e62))
* **secrets-pg:** a per-name high-water mark makes a vault version never reissued (ANR-688) ([#506](https://github.com/vymalo/another-agentic-system/issues/506)) ([be1c068](https://github.com/vymalo/another-agentic-system/commit/be1c06873a2b161d51e6c774f0a265478ac2ef1a))
* **web:** pause the dead-stream watchdog while the tab is hidden ([#453](https://github.com/vymalo/another-agentic-system/issues/453)) ([1b62770](https://github.com/vymalo/another-agentic-system/commit/1b627700c63364b3477c483f1a65e338bb133646))


### Refactoring

* **dev:** the dev stack without Podman (epic [#359](https://github.com/vymalo/another-agentic-system/issues/359), [#372](https://github.com/vymalo/another-agentic-system/issues/372)) ([#375](https://github.com/vymalo/another-agentic-system/issues/375)) ([3fe1537](https://github.com/vymalo/another-agentic-system/commit/3fe1537f666b9f5e173f4686b1eb645795e90be4))
* **locale:** neutral example tags in the locale code, tests and dev stack ([#455](https://github.com/vymalo/another-agentic-system/issues/455)) ([c026a7a](https://github.com/vymalo/another-agentic-system/commit/c026a7a4b51f21da5bd5cd641f374804a115dc06))
* **secrets-pg:** off sqlx, onto tokio-postgres ([#382](https://github.com/vymalo/another-agentic-system/issues/382) E1) ([#426](https://github.com/vymalo/another-agentic-system/issues/426)) ([a21bb2e](https://github.com/vymalo/another-agentic-system/commit/a21bb2ee86f9c009d58c32fe24959ef82bd2d720))
* **store-postgres:** off sqlx, onto tokio-postgres ([#382](https://github.com/vymalo/another-agentic-system/issues/382) E1) ([#428](https://github.com/vymalo/another-agentic-system/issues/428)) ([dbff063](https://github.com/vymalo/another-agentic-system/commit/dbff063c7ddc8005b3c6498901ba107c6b3e1ff2))


### Documentation

* a VitePress site with vitepress-md3, on GitHub Pages ([#565](https://github.com/vymalo/another-agentic-system/issues/565)) ([ef53d8e](https://github.com/vymalo/another-agentic-system/commit/ef53d8e50da067232678011591bb910d5823e100))
* ADR 0098 and contract for deleting an account ([#542](https://github.com/vymalo/another-agentic-system/issues/542)) ([9c5116e](https://github.com/vymalo/another-agentic-system/commit/9c5116e0cc22574eca2dfa8d999b59ddba22de25))
* **adr:** 0019 amended, search and read through the MCP server ([#521](https://github.com/vymalo/another-agentic-system/issues/521)) ([c0c8a68](https://github.com/vymalo/another-agentic-system/commit/c0c8a68682ce8bde3125ddf9af212a41c73097b5))
* **adr:** 0074 agent-team, several primary agents in one chat ([#378](https://github.com/vymalo/another-agentic-system/issues/378)) ([16ff595](https://github.com/vymalo/another-agentic-system/commit/16ff5951266ecbdd0a4ea7417db0e187bd0cd7ea))
* **adr:** 0075, A2UI v1.0 as a hard cutover in R1, and ui-catalog/v2 ([#381](https://github.com/vymalo/another-agentic-system/issues/381)) ([745418a](https://github.com/vymalo/another-agentic-system/commit/745418a9ae78802bddb9d79401532b9a68e0a0f2))
* **adr:** 0076, the orchestrator as a library in an application ([#383](https://github.com/vymalo/another-agentic-system/issues/383)) ([e2ebb09](https://github.com/vymalo/another-agentic-system/commit/e2ebb0950a728656bd4fc46995c992260ff6fd84))
* **adr:** 0077, push notifications for a person's runs ([#385](https://github.com/vymalo/another-agentic-system/issues/385)) ([1b60302](https://github.com/vymalo/another-agentic-system/commit/1b6030212b5806444566448e545f9b759d361021))
* **adr:** 0083 names the vault reference form, and how a rotation ages ([#432](https://github.com/vymalo/another-agentic-system/issues/432)) ([7d1051b](https://github.com/vymalo/another-agentic-system/commit/7d1051ba047e189a840d6d34f8b25a03a99ef53d))
* **adr:** 0083, trusted components — the agent names, the client draws ([#429](https://github.com/vymalo/another-agentic-system/issues/429)) ([5de27b6](https://github.com/vymalo/another-agentic-system/commit/5de27b63259e5b0dde0ccc109521604f5b1a5652))
* **adr:** 0091, a person's memory is off until they turn it on ([#495](https://github.com/vymalo/another-agentic-system/issues/495)) ([76f1e87](https://github.com/vymalo/another-agentic-system/commit/76f1e870fae74936984c449a1ad2864a77c45608))
* **adr:** 0092, a window is a port on the run's side ([#503](https://github.com/vymalo/another-agentic-system/issues/503)) ([2f06109](https://github.com/vymalo/another-agentic-system/commit/2f06109c162b51c135d1ade6f20cc434c4d79bb4))
* **adr:** 0093, a project groups repositories, instructions and defaults for chats; docs/api/projects-v1.md ([#508](https://github.com/vymalo/another-agentic-system/issues/508)) ([94d0989](https://github.com/vymalo/another-agentic-system/commit/94d09896f2316cf147d242545d5d76afb8122139))
* **adr:** 0095, the mission's ADR and missions-v1 (ANR-364) ([#518](https://github.com/vymalo/another-agentic-system/issues/518)) ([3db5ff6](https://github.com/vymalo/another-agentic-system/commit/3db5ff655462778cdddd2a3d45a4c58ce3a49f7b))
* **adr:** 0096, a held stream is a projection of the log ([#520](https://github.com/vymalo/another-agentic-system/issues/520)) ([0b7579a](https://github.com/vymalo/another-agentic-system/commit/0b7579a844cda1b4c4cca5d9c4779a40dda111a2))
* **adr:** a terminal client and a headless server as thin clients of the one API ([#544](https://github.com/vymalo/another-agentic-system/issues/544)) ([177fac9](https://github.com/vymalo/another-agentic-system/commit/177fac91d18d759a3c9b04875459b43618871739))
* **adr:** ADR 0064 accepted, the extension named identity/v1 (ANR-72) ([#476](https://github.com/vymalo/another-agentic-system/issues/476)) ([ada446c](https://github.com/vymalo/another-agentic-system/commit/ada446c064977e7812702be5e9df25dc30a86930))
* **adr:** ADR 0081 accepted; no default token allowance (ANR-622) ([#474](https://github.com/vymalo/another-agentic-system/issues/474)) ([5d1ff07](https://github.com/vymalo/another-agentic-system/commit/5d1ff0747798dc0ab9a9ab72b56565a9230fba20))
* **adr:** ADR 0084 question 4 answered — the binding names the endpoint, the card lists it (ANR-612) ([#468](https://github.com/vymalo/another-agentic-system/issues/468)) ([a4fd9a0](https://github.com/vymalo/another-agentic-system/commit/a4fd9a07dcad6bdce33788c2cb0ba278d404f34d))
* **adr:** ADR 0087, a signing app signs each prompt and the orchestrator refuses an unsigned one (ANR-576) ([#477](https://github.com/vymalo/another-agentic-system/issues/477)) ([637fa63](https://github.com/vymalo/another-agentic-system/commit/637fa63bb7c5b2c6e37f1e6c5ec5dca5886cd3dd))
* **adr:** ADR 0088, a person's schedules live in the orchestrator and each firing is a thread ([#479](https://github.com/vymalo/another-agentic-system/issues/479)) ([42ed0d2](https://github.com/vymalo/another-agentic-system/commit/42ed0d28464343655ac89813d3e95d401297dbbe))
* **adr:** ADR 0097, a deployment in several regions, with its slice table ([#523](https://github.com/vymalo/another-agentic-system/issues/523)) ([08a99ad](https://github.com/vymalo/another-agentic-system/commit/08a99ad274f72d7f02ca47b824824378f302d14b))
* **adr:** ADR 0101, a cloud run's model calls are signed for the run ([#570](https://github.com/vymalo/another-agentic-system/issues/570)) ([d50fa1d](https://github.com/vymalo/another-agentic-system/commit/d50fa1dc672ae687e559100e529da7eb4f6988ac))
* **adr:** amend 0074, bounded push, sub-agents, approval expiry ([#379](https://github.com/vymalo/another-agentic-system/issues/379)) ([8fc131f](https://github.com/vymalo/another-agentic-system/commit/8fc131f188da52e97ca12439887df2e8d0fe51f9))
* **adr:** an inspector reads a run, and replay is a fork of the journal ([#515](https://github.com/vymalo/another-agentic-system/issues/515)) ([8a41b00](https://github.com/vymalo/another-agentic-system/commit/8a41b0051c4ddda4b1a6a5356391579875e38c94))
* **adr:** cold threads are archived to object storage and rehydrated on the next message ([#564](https://github.com/vymalo/another-agentic-system/issues/564)) ([6936b91](https://github.com/vymalo/another-agentic-system/commit/6936b915b64e32b4574febd6b33b32c6ccb60cb2))
* **adr:** propose ADR 0067, the plan to LibreChat-grade and the client-agnostic rule ([#315](https://github.com/vymalo/another-agentic-system/issues/315)) ([60cb754](https://github.com/vymalo/another-agentic-system/commit/60cb75459d7226be729b8f25c605436bcd333593))
* **adr:** record the retention ages and close open questions 28, 29 and 46 ([#531](https://github.com/vymalo/another-agentic-system/issues/531)) ([ed04ba6](https://github.com/vymalo/another-agentic-system/commit/ed04ba632894ed13fade541a510a0248cd5564e6))
* **adr:** retention is one sweep per class with a key each and a default of 90 days ([#516](https://github.com/vymalo/another-agentic-system/issues/516)) ([1aa4b8c](https://github.com/vymalo/another-agentic-system/commit/1aa4b8c14377bcee986e39658110f483c30ea2d6))
* **adr:** the private repository's name and issue numbers leave ADR 0081-0083 (ANR-626) ([#471](https://github.com/vymalo/another-agentic-system/issues/471)) ([97107ff](https://github.com/vymalo/another-agentic-system/commit/97107ff1761c2948a7a79163c5f1d85587f3f327))
* amend ADR 0073 for one SQLite container per app, model pick by ADR 0086 and child sessions ([#573](https://github.com/vymalo/another-agentic-system/issues/573)) ([ca7aa81](https://github.com/vymalo/another-agentic-system/commit/ca7aa81a18bbfbe014d305c6e54db909ac986755))
* **api:** the model injector's header contract, and a stand-in test that a client cannot supply it ([#532](https://github.com/vymalo/another-agentic-system/issues/532)) ([8ff6b05](https://github.com/vymalo/another-agentic-system/commit/8ff6b051a3a48c201703c11ce324f997fd8c4000))
* **api:** ui-catalog/v2, the A2UI v1.0 catalog contract (C0 of [#380](https://github.com/vymalo/another-agentic-system/issues/380)) ([#396](https://github.com/vymalo/another-agentic-system/issues/396)) ([66db4ee](https://github.com/vymalo/another-agentic-system/commit/66db4ee5c885c12f28367a4c623f9c97c38e0930))
* close open question 45, the operator alone chooses the utility model (ANR-610) ([#475](https://github.com/vymalo/another-agentic-system/issues/475)) ([e755e1c](https://github.com/vymalo/another-agentic-system/commit/e755e1c13c5e417c7e1fec094d73dd6d7fb1021d))
* **decisions:** ADR 0084 — the run lease (B1's design) ([#436](https://github.com/vymalo/another-agentic-system/issues/436)) ([58fdcbc](https://github.com/vymalo/another-agentic-system/commit/58fdcbc104f9615d74b5ede578de15fe9a419a29))
* **decisions:** ADR 0084 decided — the deployment's key, a 30 s lease ([#440](https://github.com/vymalo/another-agentic-system/issues/440)) ([a453d21](https://github.com/vymalo/another-agentic-system/commit/a453d218c862a787c4199dceb2233b0f1be49604))
* **decisions:** ADR 0089, the run identity stays the agent's task id (proposed) ([#480](https://github.com/vymalo/another-agentic-system/issues/480)) ([eddee8f](https://github.com/vymalo/another-agentic-system/commit/eddee8f9cc800f507294facc54c3dffdee8a1fa2))
* **decisions:** ADR 0090, published clients of the chat API (proposed) ([#482](https://github.com/vymalo/another-agentic-system/issues/482)) ([f035fd6](https://github.com/vymalo/another-agentic-system/commit/f035fd68cab40206b23c74f792b4dc9b342cd84b))
* **decisions:** limits per person before a run starts (ADR 0081) ([#424](https://github.com/vymalo/another-agentic-system/issues/424)) ([82e4a33](https://github.com/vymalo/another-agentic-system/commit/82e4a336951850c280871098d3f23d2cf1ca7b8c))
* gVisor is back in milestone M1, and the goal rows are current ([#326](https://github.com/vymalo/another-agentic-system/issues/326)) ([3e3bf12](https://github.com/vymalo/another-agentic-system/commit/3e3bf128594583f3bea9c2d01d7b45d590fe262f))
* **model-pick:** ADR 0086 and the model-pick/v1 contract ([#450](https://github.com/vymalo/another-agentic-system/issues/450)) ([9029e69](https://github.com/vymalo/another-agentic-system/commit/9029e699ee3041e8237ba3cb6c8fce5b6b055b3b))
* **open-questions:** move question 6 to ADR 0081, recommend waiting on question 27 (ANR-331) ([#513](https://github.com/vymalo/another-agentic-system/issues/513)) ([32fdb60](https://github.com/vymalo/another-agentic-system/commit/32fdb60b5e9470c6bdf397e6bf453bb82a31b3a8))
* **orchestrator:** the two composition roots, and the embedded one in the system diagram ([#473](https://github.com/vymalo/another-agentic-system/issues/473)) ([797073f](https://github.com/vymalo/another-agentic-system/commit/797073f98dfa291fa1fd4a78f516a676d187dc22))
* payments, pricing and the market plan leave the public roadmap ([#327](https://github.com/vymalo/another-agentic-system/issues/327)) ([50650e6](https://github.com/vymalo/another-agentic-system/commit/50650e6fd1f01ed9b2727ab43bb3136b64cadaa0))
* pgvector for embeddings and vector search, behind a port (ADR 0071) ([#329](https://github.com/vymalo/another-agentic-system/issues/329)) ([291379f](https://github.com/vymalo/another-agentic-system/commit/291379f1156776af3f996026c499fb1843931c9f))
* **plan:** the app-factory board and model selection (ADR 0073) ([#358](https://github.com/vymalo/another-agentic-system/issues/358)) ([78bbe49](https://github.com/vymalo/another-agentic-system/commit/78bbe49bdb8f8a0b9b0b7fe6b0b0d768af412ad9))
* record the owner's PKI and orchestrator-signing answers on the signing ADRs ([#572](https://github.com/vymalo/another-agentic-system/issues/572)) ([e81be7a](https://github.com/vymalo/another-agentic-system/commit/e81be7ae50bfae2b8733e7333a9aa34687379841))
* relations beside the vectors, in two plain tables (ADR 0071 amended) ([#330](https://github.com/vymalo/another-agentic-system/issues/330)) ([cf33afa](https://github.com/vymalo/another-agentic-system/commit/cf33afa651c664606bbcaf902272b0524aa09e4e))
* render the chat API on the docs site with vitepress-openapi (ANR-922) ([#593](https://github.com/vymalo/another-agentic-system/issues/593)) ([5e6dff4](https://github.com/vymalo/another-agentic-system/commit/5e6dff48bfe5d3447f7360cef629fa8a0b9da542))
* **roadmap:** A2UI v1.0 is 7 of 7, S4 merged in sys[#418](https://github.com/vymalo/another-agentic-system/issues/418) ([#422](https://github.com/vymalo/another-agentic-system/issues/422)) ([3d1fdb1](https://github.com/vymalo/another-agentic-system/commit/3d1fdb152fe89f76bb3430aa7f85073f47fc74f8))
* **roadmap:** adam-rs is 16 of 17, images in is merged ([#331](https://github.com/vymalo/another-agentic-system/issues/331)) ([661b7da](https://github.com/vymalo/another-agentic-system/commit/661b7da18287f290b20c181d7527f3f9c1ff4aed))
* **roadmap:** background sessions 3/8, LibreChat-grade 12/35, kagent-grade 9/16 ([#354](https://github.com/vymalo/another-agentic-system/issues/354)) ([f3e5f57](https://github.com/vymalo/another-agentic-system/commit/f3e5f57e5956f6b0a20f85f7832ccd58bc7e16ac))
* **roadmap:** link the epics of M6 to M11 ([#408](https://github.com/vymalo/another-agentic-system/issues/408)) ([fd5d814](https://github.com/vymalo/another-agentic-system/commit/fd5d814a5599d0eebbb7e83b36cf0a6a8fab028f))
* **roadmap:** progress per goal and step, checked 2026-11-21 ([#420](https://github.com/vymalo/another-agentic-system/issues/420)) ([4e2ac02](https://github.com/vymalo/another-agentic-system/commit/4e2ac0278e40be09edbe2868e418e0c91f8f9033))
* **roadmap:** propose a release sequence, R1 to R3, with what waits ([#376](https://github.com/vymalo/another-agentic-system/issues/376)) ([7eb661f](https://github.com/vymalo/another-agentic-system/commit/7eb661f50ad63ff928d6e4a50a73817798443f5d))
* **roadmap:** say what M2 has built (usage ledger, sandbox time, fair share, edge limit) ([#308](https://github.com/vymalo/another-agentic-system/issues/308)) ([6b1d0f0](https://github.com/vymalo/another-agentic-system/commit/6b1d0f0fb1a7282a5f23e96d3cf8805a1564ad6a))
* **roadmap:** six milestones from Rivet's feature list, M6 to M11 ([#401](https://github.com/vymalo/another-agentic-system/issues/401)) ([c2c1383](https://github.com/vymalo/another-agentic-system/commit/c2c1383e6a2a17b2ba48c71cd693a7d9236d9e3c))
* **roadmap:** the coder plan is 17 of 17; one epic per track ([#346](https://github.com/vymalo/another-agentic-system/issues/346)) ([470e156](https://github.com/vymalo/another-agentic-system/commit/470e1564a30e0c9d0bfee545fb9ded04ee2adfe8))
* **roadmap:** the releases agreed, embedded adam exempt, the counts of 2026-11-19 ([#386](https://github.com/vymalo/another-agentic-system/issues/386)) ([1092ee2](https://github.com/vymalo/another-agentic-system/commit/1092ee220430b75ee6de7e72671fa5f4c45fd437))
* **roadmap:** two production layers, adam-rs, agents and A2UI at the centre ([#391](https://github.com/vymalo/another-agentic-system/issues/391)) ([536b6b1](https://github.com/vymalo/another-agentic-system/commit/536b6b19a3af23eb810521863067eaec29e75234))
* structure v2 of the docs site and its sidebar (ANR-920) ([#592](https://github.com/vymalo/another-agentic-system/issues/592)) ([c791f17](https://github.com/vymalo/another-agentic-system/commit/c791f1722231f6cef0128025a888180dfb6a4068))
* the extensions index says what is built, with the missing rows ([c6d3de6](https://github.com/vymalo/another-agentic-system/commit/c6d3de628587e45f611d5d54a52e4c419de2dae3))
* the vendor check for an outside worker (missions slice 1) ([#483](https://github.com/vymalo/another-agentic-system/issues/483)) ([c5e75ee](https://github.com/vymalo/another-agentic-system/commit/c5e75ee8fcac67e7aeb7efb654592d50651308d1))
* two sentences name no head count ([#328](https://github.com/vymalo/another-agentic-system/issues/328)) ([bd204d6](https://github.com/vymalo/another-agentic-system/commit/bd204d6d20fb6ed3772f7d1af33774d382705e9d))
* use neutral locale examples in the locale/v1 spec and ADRs ([#447](https://github.com/vymalo/another-agentic-system/issues/447)) ([6683fae](https://github.com/vymalo/another-agentic-system/commit/6683fae1b4dbe51e9e1c58325d1b6cdd40791c09))
* **vendor-check:** the second pass, with what was read and what stays unverified (ANR-631) ([#504](https://github.com/vymalo/another-agentic-system/issues/504)) ([f444bca](https://github.com/vymalo/another-agentic-system/commit/f444bca399793cc84ffd351ae9e20af768ee1197))


### Tests

* **agent-adam:** the embedded and the remote path give equal A2A envelopes ([#571](https://github.com/vymalo/another-agentic-system/issues/571)) ([90c4415](https://github.com/vymalo/another-agentic-system/commit/90c44154995abc73d3410fa3b660a04617770f0a))
* **chart:** the database goldens carry the pinned CNPG image ([#578](https://github.com/vymalo/another-agentic-system/issues/578)) ([80a7b95](https://github.com/vymalo/another-agentic-system/commit/80a7b9528d849dbf5b25b13c45588a973b0958f7))
* **dev:** a load test of the dev stack's mocks, one command, ramping concurrent runs ([#524](https://github.com/vymalo/another-agentic-system/issues/524)) ([a313231](https://github.com/vymalo/another-agentic-system/commit/a31323122eadfac87e0a23dd520a55de4bbc93dc))
* **dev:** child sessions end to end on mocks (S2f) ([#357](https://github.com/vymalo/another-agentic-system/issues/357)) ([118907e](https://github.com/vymalo/another-agentic-system/commit/118907e92e5384593853ffa23de12c765dea78a6))
* **dev:** prompts-e2e gains an MCP leg for prompts/list and prompts/get ([#502](https://github.com/vymalo/another-agentic-system/issues/502)) ([96bbc04](https://github.com/vymalo/another-agentic-system/commit/96bbc046c1e1ffc69824d6dc0220a73580b74e13))
* **dev:** prove the model pick on the real adam (pick-real-e2e) ([#587](https://github.com/vymalo/another-agentic-system/issues/587)) ([a3aceef](https://github.com/vymalo/another-agentic-system/commit/a3aceef1f9f1874c9ce1d0de33cbfe85b51edfbb))
* **dev:** search-e2e says what matched when the mid-word prefix check fails ([#445](https://github.com/vymalo/another-agentic-system/issues/445)) ([049767b](https://github.com/vymalo/another-agentic-system/commit/049767b4eef9a0745310118fb823645acb1c34b8))
* **dev:** the interface language on the real chat agent, end to end ([#456](https://github.com/vymalo/another-agentic-system/issues/456)) ([a1a01a2](https://github.com/vymalo/another-agentic-system/commit/a1a01a2da855ce558b3b78fd98317850773d241d))
* **dev:** the pick e2e compares against sorted keys, as jq -cS writes them (ANR-625) ([#469](https://github.com/vymalo/another-agentic-system/issues/469)) ([a58ed8a](https://github.com/vymalo/another-agentic-system/commit/a58ed8a036b9358d08c7d146981cba4082c017c9))
* **e2e:** wait on the event log for an unreachable agent ([#519](https://github.com/vymalo/another-agentic-system/issues/519)) ([466c35a](https://github.com/vymalo/another-agentic-system/commit/466c35a4afa99b1a8003c402cb153d11aee2caf2))
* **embed:** a killed embedded application resumes its thread and its local run ([#576](https://github.com/vymalo/another-agentic-system/issues/576)) ([de56950](https://github.com/vymalo/another-agentic-system/commit/de56950fe032067bbacbbb71cf9693d07603a476))
* **provisioner:** prove the AgentProvisioner testkit can fail and narrow create's 409 ([#551](https://github.com/vymalo/another-agentic-system/issues/551)) ([d8a4243](https://github.com/vymalo/another-agentic-system/commit/d8a4243fc7c942164279ed06c6dd57513d440580))
* **web:** settle the still before the shot, and re-baseline the screens from the image ([#589](https://github.com/vymalo/another-agentic-system/issues/589)) ([fad481f](https://github.com/vymalo/another-agentic-system/commit/fad481fa8214e25f49729b7467b3ee111bb88486))
* **web:** the child session's stream test waits for the session, not the clock ([#419](https://github.com/vymalo/another-agentic-system/issues/419)) ([4a53090](https://github.com/vymalo/another-agentic-system/commit/4a53090eb52efb5a7d29c4dd5d607c0fdc80fbb2))
* **web:** wait a frame after a mention pick so the caret has moved ([#584](https://github.com/vymalo/another-agentic-system/issues/584)) ([7273ebc](https://github.com/vymalo/another-agentic-system/commit/7273ebc23b3ff18fdbeff4c311a17239c43f0f6e))
* **web:** wait for each mention pick to land before typing on in the screens spec ([#579](https://github.com/vymalo/another-agentic-system/issues/579)) ([5ecbc4f](https://github.com/vymalo/another-agentic-system/commit/5ecbc4f07f9a85458b9da92766db416bcc792dca))


### Build System

* adam-rs 7ac3281 for the crates and the coder image, sqlx out of the orchestrator ([#382](https://github.com/vymalo/another-agentic-system/issues/382) E1) ([#435](https://github.com/vymalo/another-agentic-system/issues/435)) ([26126c7](https://github.com/vymalo/another-agentic-system/commit/26126c74abe13c1b70d823735faba30d07092e28))
* **agent-adam:** wire adam-engine and adam-store-sqlite behind an embedded feature ([#545](https://github.com/vymalo/another-agentic-system/issues/545)) ([705d0af](https://github.com/vymalo/another-agentic-system/commit/705d0aff48e2bc4c334ba782cfaa0c85db39635d))
* bump adam-rs to 7418467 (coder sha-7418467) ([#397](https://github.com/vymalo/another-agentic-system/issues/397)) ([868f84a](https://github.com/vymalo/another-agentic-system/commit/868f84a5ef557ec5775ded67647f8255fc3ca93d))
* **compose:** pin the coder to adam-rs 9f90f1e (model pick, reasoning, prompt statement) ([#586](https://github.com/vymalo/another-agentic-system/issues/586)) ([e501f2c](https://github.com/vymalo/another-agentic-system/commit/e501f2c57ddb54875ab05fafdd9aec777e262a3d))
* **deps:** PostgreSQL 19.0 in CI and the local stack, pinned CNPG standard image in the chart ([#567](https://github.com/vymalo/another-agentic-system/issues/567)) ([a219682](https://github.com/vymalo/another-agentic-system/commit/a219682c3a12d1b68ec5786b734e3feeb57db01d))
* pin the coder to adam-rs 8501494 (A2UI v1.0 and [#273](https://github.com/vymalo/another-agentic-system/issues/273)) ([#418](https://github.com/vymalo/another-agentic-system/issues/418)) ([9654158](https://github.com/vymalo/another-agentic-system/commit/9654158d458617f4519450e967ffc2a816c2bbbc))


### Continuous Integration

* **api:** fail a pull request that breaks a client of chat-api.yaml (ANR-761) ([#540](https://github.com/vymalo/another-agentic-system/issues/540)) ([76afeb3](https://github.com/vymalo/another-agentic-system/commit/76afeb32fd0b842b66d219a6c5f75b8647d74950))
* **deploy:** azure/setup-helm moves to the major that runs on node24 ([#485](https://github.com/vymalo/another-agentic-system/issues/485)) ([dc75af3](https://github.com/vymalo/another-agentic-system/commit/dc75af3ec659cd06449a440cafb0448a81c451b5))
* **embed:** the embedded build's dependency tree holds no server and one SQLite driver (ANR-390) ([#472](https://github.com/vymalo/another-agentic-system/issues/472)) ([136463d](https://github.com/vymalo/another-agentic-system/commit/136463d32d548c400115d3271400b871bdc2c0bf))
* **web:** generate the committed screenshots in the web image and fail on drift (ANR-687) ([#507](https://github.com/vymalo/another-agentic-system/issues/507)) ([d23db8a](https://github.com/vymalo/another-agentic-system/commit/d23db8a151ca749967ba1fd927194951885e9bf9))


### Chores

* **deploy:** bump orchestrator to sha-00a4a30 ([25f4baf](https://github.com/vymalo/another-agentic-system/commit/25f4bafa667e1d5561fa5e66441ea3cf5e26f368))
* **deploy:** bump orchestrator to sha-05c8acc ([2ae76bf](https://github.com/vymalo/another-agentic-system/commit/2ae76bfbbc41ca9670c8e5aca6aac2ef56168d3b))
* **deploy:** bump orchestrator to sha-0ccff4f ([8452d65](https://github.com/vymalo/another-agentic-system/commit/8452d657ed8947a00ea3778efbbf53397ac7e11c))
* **deploy:** bump orchestrator to sha-0d50c42 ([795ebe8](https://github.com/vymalo/another-agentic-system/commit/795ebe8d6ca1bfa6f42f74ec185b8c9e83068d9a))
* **deploy:** bump orchestrator to sha-118907e ([84ea30f](https://github.com/vymalo/another-agentic-system/commit/84ea30f92df18451bfe8fbe9dd26b63b72b5bd79))
* **deploy:** bump orchestrator to sha-1c70dd7 ([2ab6991](https://github.com/vymalo/another-agentic-system/commit/2ab69911a0e98b6dcfc8006bbe6cf7646e2d689b))
* **deploy:** bump orchestrator to sha-1fd60d0 ([2a3f460](https://github.com/vymalo/another-agentic-system/commit/2a3f460d8bc259d8ac4ec59cd8c9fe150792bdcf))
* **deploy:** bump orchestrator to sha-2273c9b ([55bd07f](https://github.com/vymalo/another-agentic-system/commit/55bd07f69fb26f698e0544a6f0cbcc0e04a3ba3a))
* **deploy:** bump orchestrator to sha-2522fcd ([744e8a9](https://github.com/vymalo/another-agentic-system/commit/744e8a99c35b43099bba8f70ce006ef36c0872fe))
* **deploy:** bump orchestrator to sha-26126c7 ([435fb5a](https://github.com/vymalo/another-agentic-system/commit/435fb5a3ad52666d0bfe614dc5dc8d4d176db11d))
* **deploy:** bump orchestrator to sha-2654109 ([17ff793](https://github.com/vymalo/another-agentic-system/commit/17ff7935a8e57eb2cf90a236574b618ab6068eb2))
* **deploy:** bump orchestrator to sha-26d9c17 ([9afaad3](https://github.com/vymalo/another-agentic-system/commit/9afaad3258c098e09794d083a3f3f79821c5450c))
* **deploy:** bump orchestrator to sha-324ed15 ([408566c](https://github.com/vymalo/another-agentic-system/commit/408566cdbe51f922a9e5279d02f46e28de6b0f0b))
* **deploy:** bump orchestrator to sha-35d1001 ([3c36b6a](https://github.com/vymalo/another-agentic-system/commit/3c36b6a3ed01e38e20b107396c02936e105d8e44))
* **deploy:** bump orchestrator to sha-4268e9d ([efcef2f](https://github.com/vymalo/another-agentic-system/commit/efcef2fc04f0d492173499d466f2ce58bc355b16))
* **deploy:** bump orchestrator to sha-466c35a ([75d8cb8](https://github.com/vymalo/another-agentic-system/commit/75d8cb81b9d3ce75c9b30f0790da1efa57c042c0))
* **deploy:** bump orchestrator to sha-469e1ee ([d1fcf83](https://github.com/vymalo/another-agentic-system/commit/d1fcf83ae9932fc132bb7d68f8af5a371769157e))
* **deploy:** bump orchestrator to sha-47bda3a ([ae3af9f](https://github.com/vymalo/another-agentic-system/commit/ae3af9f0dd15756642da3d8387a307ae173bafa4))
* **deploy:** bump orchestrator to sha-4b6c5b7 ([e7e77c7](https://github.com/vymalo/another-agentic-system/commit/e7e77c78248e74f20d439f0731d3f7e1175013a3))
* **deploy:** bump orchestrator to sha-4d7c915 ([e7ea2ad](https://github.com/vymalo/another-agentic-system/commit/e7ea2ad223d2d2fd2047f53edc02676e1ae9d450))
* **deploy:** bump orchestrator to sha-5cab8fc ([794050e](https://github.com/vymalo/another-agentic-system/commit/794050e62079b75da13b6919b0dd0ba1338ec8f2))
* **deploy:** bump orchestrator to sha-5ed8950 ([dbeeca1](https://github.com/vymalo/another-agentic-system/commit/dbeeca168c9debcd7371798945bf0c4dea5f5e29))
* **deploy:** bump orchestrator to sha-6105fe2 ([190d1a5](https://github.com/vymalo/another-agentic-system/commit/190d1a55d9ad1774c1b64963253e88690788d7b0))
* **deploy:** bump orchestrator to sha-6224b9c ([15cb544](https://github.com/vymalo/another-agentic-system/commit/15cb5447dc2bd42b3087be0c1bf2a6cb48762e99))
* **deploy:** bump orchestrator to sha-6739a06 ([bbdbb4b](https://github.com/vymalo/another-agentic-system/commit/bbdbb4bfe6ad862fa54f26cd5f7ac2e5d4f19f03))
* **deploy:** bump orchestrator to sha-67455b7 ([a3a3bb7](https://github.com/vymalo/another-agentic-system/commit/a3a3bb72c9d4c679c782b9149d58eb6b01f9c3ff))
* **deploy:** bump orchestrator to sha-6a64770 ([f80511b](https://github.com/vymalo/another-agentic-system/commit/f80511b03c32ca4999b00c3eff369611304d672d))
* **deploy:** bump orchestrator to sha-6e75e47 ([45bc2c0](https://github.com/vymalo/another-agentic-system/commit/45bc2c0d4b28184243efffd7117fb5fe657c4e44))
* **deploy:** bump orchestrator to sha-6fc8060 ([61a9cd6](https://github.com/vymalo/another-agentic-system/commit/61a9cd665b316e3453304a2f3d874c453a70ea2a))
* **deploy:** bump orchestrator to sha-714da1e ([a67dae7](https://github.com/vymalo/another-agentic-system/commit/a67dae7ad4905eb1cf77dbbd68dc79c0eb18762c))
* **deploy:** bump orchestrator to sha-716c260 ([cb3a7f9](https://github.com/vymalo/another-agentic-system/commit/cb3a7f9e7c45a9b76351cc286c61c954cd36332c))
* **deploy:** bump orchestrator to sha-797073f ([72755bc](https://github.com/vymalo/another-agentic-system/commit/72755bcb8ee912c68d7a59594f5f1c4bc0420ccc))
* **deploy:** bump orchestrator to sha-7a5044d ([a7e1eb8](https://github.com/vymalo/another-agentic-system/commit/a7e1eb8afcd30eb62ee169ae3c2423f7da78009a))
* **deploy:** bump orchestrator to sha-7a509cb ([72c735f](https://github.com/vymalo/another-agentic-system/commit/72c735fff79fff5f438398940e51969dfdd9bd10))
* **deploy:** bump orchestrator to sha-7fcad12 ([cb58255](https://github.com/vymalo/another-agentic-system/commit/cb58255c8e1acbf6fedae72b4dfba9199c71d4a0))
* **deploy:** bump orchestrator to sha-868f84a ([01f51a9](https://github.com/vymalo/another-agentic-system/commit/01f51a96203c3b34d4833bbc8b8265a8908fb1e6))
* **deploy:** bump orchestrator to sha-98d6d61 ([d49dd31](https://github.com/vymalo/another-agentic-system/commit/d49dd310af7fa91bd6c5c4e3e4a9c387fb2bd99a))
* **deploy:** bump orchestrator to sha-9febb68 ([3658cee](https://github.com/vymalo/another-agentic-system/commit/3658cee03c735399d1fbbc823b7a2d3f03866e4f))
* **deploy:** bump orchestrator to sha-a079e92 ([85ff9b5](https://github.com/vymalo/another-agentic-system/commit/85ff9b55f335b0ccb8fe84f4c4941edb3cdf30dd))
* **deploy:** bump orchestrator to sha-a21bb2e ([cf6ed05](https://github.com/vymalo/another-agentic-system/commit/cf6ed058bece89b44302040db3cee7e9ed6ebff2))
* **deploy:** bump orchestrator to sha-a900bc3 ([392a156](https://github.com/vymalo/another-agentic-system/commit/392a1569225885d02e59f20e2396ca9baf278c7a))
* **deploy:** bump orchestrator to sha-af1a5f1 ([536577a](https://github.com/vymalo/another-agentic-system/commit/536577ae1c9af6df4f19884038b3f9b7a3482738))
* **deploy:** bump orchestrator to sha-b35ecc3 ([ff2c911](https://github.com/vymalo/another-agentic-system/commit/ff2c91110a703b6013c72f3709cc3a12fbbba507))
* **deploy:** bump orchestrator to sha-b8b112f ([c1b2593](https://github.com/vymalo/another-agentic-system/commit/c1b2593f833863465eea6c12f6ef146370033817))
* **deploy:** bump orchestrator to sha-b97de39 ([ad2326f](https://github.com/vymalo/another-agentic-system/commit/ad2326f5508b61839f2f8994d1277d6437efd69d))
* **deploy:** bump orchestrator to sha-ba532a6 ([db2a3cd](https://github.com/vymalo/another-agentic-system/commit/db2a3cd81e1f4139e7437adb2006f6d57c86530e))
* **deploy:** bump orchestrator to sha-bb4d722 ([438509a](https://github.com/vymalo/another-agentic-system/commit/438509a6d044593170b78115d943dafcc9f7e0d9))
* **deploy:** bump orchestrator to sha-c026a7a ([8d586e1](https://github.com/vymalo/another-agentic-system/commit/8d586e140620116cadfa4b2d61bf7c232df88f35))
* **deploy:** bump orchestrator to sha-c3438b3 ([ddc1cf7](https://github.com/vymalo/another-agentic-system/commit/ddc1cf7aca240efd96ac2e0137f8f0f89963c944))
* **deploy:** bump orchestrator to sha-c777d29 ([ab49f87](https://github.com/vymalo/another-agentic-system/commit/ab49f877494a853af2efd4b217fbe23e0b0ef665))
* **deploy:** bump orchestrator to sha-d0a732f ([a548f06](https://github.com/vymalo/another-agentic-system/commit/a548f0623b605e0a6a202ddb5ad1db3fcaee645e))
* **deploy:** bump orchestrator to sha-d0b2dec ([0fc8a0a](https://github.com/vymalo/another-agentic-system/commit/0fc8a0ad99a8631e4d9f7713df0adba9ac35ee9b))
* **deploy:** bump orchestrator to sha-d5e48f5 ([d416a96](https://github.com/vymalo/another-agentic-system/commit/d416a9601cb3f691b71d444f5364b27f99b40bdc))
* **deploy:** bump orchestrator to sha-d8a4243 ([32671f8](https://github.com/vymalo/another-agentic-system/commit/32671f818f8606367689f2dcb93b00bf0b09278f))
* **deploy:** bump orchestrator to sha-de56950 ([199fabf](https://github.com/vymalo/another-agentic-system/commit/199fabf4db1c1e5b440ad50453e46fcae0223bc9))
* **deploy:** bump orchestrator to sha-e2891dc ([3ff5c73](https://github.com/vymalo/another-agentic-system/commit/3ff5c73a55bca136a07d67e01fdc7efb363de1d4))
* **deploy:** bump orchestrator to sha-e4080c7 ([866ff3a](https://github.com/vymalo/another-agentic-system/commit/866ff3a6b23dc446421c9f477cff7ff611c954bd))
* **deploy:** bump orchestrator to sha-e501f2c ([9795cd9](https://github.com/vymalo/another-agentic-system/commit/9795cd97aca0474d67dfa8ce5e89de28d6b3b4b7))
* **deploy:** bump orchestrator to sha-e95eafc ([bb1648b](https://github.com/vymalo/another-agentic-system/commit/bb1648b1c520aca09c703703f72765f1e7a72b22))
* **deploy:** bump orchestrator to sha-eb0bed7 ([04df012](https://github.com/vymalo/another-agentic-system/commit/04df01281146b7e15e43888da478d2d7a13dcb42))
* **deploy:** bump orchestrator to sha-ebcee35 ([52747ea](https://github.com/vymalo/another-agentic-system/commit/52747ea795703768b9de3fe2ae3a3092bef8eb87))
* **deploy:** bump orchestrator to sha-f3ea64f ([c03035c](https://github.com/vymalo/another-agentic-system/commit/c03035cedf5ccafd3160526636035302ea34ad2b))
* **deploy:** bump orchestrator to sha-fe055cb ([ae4b54d](https://github.com/vymalo/another-agentic-system/commit/ae4b54d94c0c2c144e529556619b5127d8d01689))
* **deploy:** bump web to sha-00a4a30 ([cd70908](https://github.com/vymalo/another-agentic-system/commit/cd70908f54bcd740fdf96906037cfd188a47adf3))
* **deploy:** bump web to sha-05c8acc ([b2ab38e](https://github.com/vymalo/another-agentic-system/commit/b2ab38e8664b029695b78e3b53b3e0e21d727062))
* **deploy:** bump web to sha-2273c9b ([97876ff](https://github.com/vymalo/another-agentic-system/commit/97876ffbf81700a389da20ff39e3185d70859507))
* **deploy:** bump web to sha-2522fcd ([e93f7b5](https://github.com/vymalo/another-agentic-system/commit/e93f7b5cc515b032e3629a1e8e5f2f6242ce8c32))
* **deploy:** bump web to sha-37600d7 ([d70436d](https://github.com/vymalo/another-agentic-system/commit/d70436d361317b0edb91be4ab04aa03f96e754ff))
* **deploy:** bump web to sha-3875e07 ([0c96dcf](https://github.com/vymalo/another-agentic-system/commit/0c96dcfaad2bd102cc4e17e3fb7e5c41fdd7ef00))
* **deploy:** bump web to sha-49eef15 ([a9b21cb](https://github.com/vymalo/another-agentic-system/commit/a9b21cbb8851618dafd676a5401e06e2c0260cbf))
* **deploy:** bump web to sha-4a53090 ([34c9e29](https://github.com/vymalo/another-agentic-system/commit/34c9e291284126d8ac999e3a39cfe9de8b0ab89b))
* **deploy:** bump web to sha-4d7c915 ([306da5b](https://github.com/vymalo/another-agentic-system/commit/306da5ba3a1f2974511c86d05ca8d59b7bf15f65))
* **deploy:** bump web to sha-5cab8fc ([36a9abb](https://github.com/vymalo/another-agentic-system/commit/36a9abbc2a3c9c04e9fbab365a0f2017f6dae592))
* **deploy:** bump web to sha-5d1b830 ([81ef618](https://github.com/vymalo/another-agentic-system/commit/81ef618d476749b84c32de97660ea5aa09f9bfab))
* **deploy:** bump web to sha-5ecbc4f ([d545227](https://github.com/vymalo/another-agentic-system/commit/d5452278ee716763b339f21a8b443078756fe0eb))
* **deploy:** bump web to sha-5ed8950 ([5b8f009](https://github.com/vymalo/another-agentic-system/commit/5b8f0090256c0017d0042d6e1c8a6d28990012fd))
* **deploy:** bump web to sha-632384f ([542505c](https://github.com/vymalo/another-agentic-system/commit/542505cc975616f883d312e7cda6ce4b57493614))
* **deploy:** bump web to sha-67455b7 ([2b0d32a](https://github.com/vymalo/another-agentic-system/commit/2b0d32ab112c96e88b652bcf23544098ce9697fd))
* **deploy:** bump web to sha-6e75e47 ([e4a7de7](https://github.com/vymalo/another-agentic-system/commit/e4a7de7f9743a1010e048170bfff59426f0da737))
* **deploy:** bump web to sha-6fc8060 ([6b1ab65](https://github.com/vymalo/another-agentic-system/commit/6b1ab65c79f4367180ea5ad0aabf16212ee3eb3e))
* **deploy:** bump web to sha-716c260 ([fafb16d](https://github.com/vymalo/another-agentic-system/commit/fafb16dd90b9a2bce78961f65949eff441349683))
* **deploy:** bump web to sha-71d1fe2 ([8fa5d75](https://github.com/vymalo/another-agentic-system/commit/8fa5d757cfd7a1a5ddfb1fd12610efc4d81c56a8))
* **deploy:** bump web to sha-7273ebc ([e9c6f52](https://github.com/vymalo/another-agentic-system/commit/e9c6f52a9af7ec3b4ce12d91f83b13b48d190677))
* **deploy:** bump web to sha-729dbdb ([f9f25cb](https://github.com/vymalo/another-agentic-system/commit/f9f25cba20b61b60192cf17fb70e7f28a7fd316e))
* **deploy:** bump web to sha-76f1e87 ([f7d1c2f](https://github.com/vymalo/another-agentic-system/commit/f7d1c2fe026cafdd156bc3cace4f7b14c23fb9b2))
* **deploy:** bump web to sha-89762da ([5f23c59](https://github.com/vymalo/another-agentic-system/commit/5f23c59fa16bcdb48cb67ce1cc66085c1295c00e))
* **deploy:** bump web to sha-92613d5 ([2443684](https://github.com/vymalo/another-agentic-system/commit/2443684d033345cf08b178706efef28c577f7ee6))
* **deploy:** bump web to sha-94d0989 ([f005588](https://github.com/vymalo/another-agentic-system/commit/f00558868d03747f07d31c2edd182c2aedd7d9a1))
* **deploy:** bump web to sha-98d6d61 ([cf4c4e0](https://github.com/vymalo/another-agentic-system/commit/cf4c4e0f294fcca4a21eea357399608afd125e12))
* **deploy:** bump web to sha-9c626db ([3212875](https://github.com/vymalo/another-agentic-system/commit/3212875e0253768884a9bd17b7848e18a9a466df))
* **deploy:** bump web to sha-9febb68 ([5e54e00](https://github.com/vymalo/another-agentic-system/commit/5e54e0043a8621e3baf1e65f93425e60ba78c0e8))
* **deploy:** bump web to sha-a900bc3 ([e98be7f](https://github.com/vymalo/another-agentic-system/commit/e98be7fb05bc2228d0cbda697aad7853f35c6e8c))
* **deploy:** bump web to sha-af1a5f1 ([3761d29](https://github.com/vymalo/another-agentic-system/commit/3761d29044b4ee00e7c9527ef11fc06c7491a30c))
* **deploy:** bump web to sha-af95948 ([9dc6907](https://github.com/vymalo/another-agentic-system/commit/9dc6907a75b0b98ebf4f663d277d8973d03fa639))
* **deploy:** bump web to sha-b79132a ([d278240](https://github.com/vymalo/another-agentic-system/commit/d278240cfdf03d3dc2f3f014897392260a273999))
* **deploy:** bump web to sha-b97de39 ([e978437](https://github.com/vymalo/another-agentic-system/commit/e97843743226fe404c60262ec09c8624feb55e3b))
* **deploy:** bump web to sha-bb4d722 ([f0a2745](https://github.com/vymalo/another-agentic-system/commit/f0a2745bff7c57d52143b289fcb3ec565fb656c7))
* **deploy:** bump web to sha-d0a732f ([c860609](https://github.com/vymalo/another-agentic-system/commit/c860609c5a7862b46f91a86b7345b141b3b19e73))
* **deploy:** bump web to sha-d23db8a ([ab1aa4a](https://github.com/vymalo/another-agentic-system/commit/ab1aa4a85d1351eb68a24c9dbe1ee95692235981))
* **deploy:** bump web to sha-d5e48f5 ([59c5ce5](https://github.com/vymalo/another-agentic-system/commit/59c5ce53d8527c546c0003f7ae8493cdf4f7b82d))
* **deploy:** bump web to sha-e2891dc ([b2e2d4b](https://github.com/vymalo/another-agentic-system/commit/b2e2d4be5a78e4669cb5813d631079a6f37c249e))
* **deploy:** bump web to sha-e4080c7 ([cd4a030](https://github.com/vymalo/another-agentic-system/commit/cd4a030308c3fd04b47f500becb6fc3daf4fdeb2))
* **deploy:** bump web to sha-e9b11c5 ([d7bd017](https://github.com/vymalo/another-agentic-system/commit/d7bd017afbc803d0ec319f3f7c64bbabb2a563ef))
* **deploy:** bump web to sha-eb0bed7 ([eaecb7b](https://github.com/vymalo/another-agentic-system/commit/eaecb7bd00dd87da04194b2fcd5101402c72502b))
* **deploy:** bump web to sha-f367469 ([2527b34](https://github.com/vymalo/another-agentic-system/commit/2527b34d5ca52516b7accff4692892bce1515d4d))
* **deploy:** bump web to sha-f63fde1 ([7578dfc](https://github.com/vymalo/another-agentic-system/commit/7578dfc907b01b5db21dc2b0a97c70c536275586))
* **deploy:** bump web to sha-fc9b293 ([4417d7f](https://github.com/vymalo/another-agentic-system/commit/4417d7fc9ed6c75febc33445bbdb64ff6766646b))
* **deploy:** bump web to sha-fe055cb ([ce1c62f](https://github.com/vymalo/another-agentic-system/commit/ce1c62fba28da09ad4f2a8011b8b3823aa4f5ad6))
* **dev:** an api-docs profile with Swagger UI and Swagger Editor over every OpenAPI document ([#559](https://github.com/vymalo/another-agentic-system/issues/559)) ([2714ed6](https://github.com/vymalo/another-agentic-system/commit/2714ed6e8bae0b8a9d3537fcfc3175c48e583fb9))
* **dev:** drop Swagger Editor from the api-docs profile ([#561](https://github.com/vymalo/another-agentic-system/issues/561)) ([1e07c8f](https://github.com/vymalo/another-agentic-system/commit/1e07c8febafceb65178fc901adcd6f2d9e64921c))
* **skills:** update the vendored skills, and bump-adam names them ([#421](https://github.com/vymalo/another-agentic-system/issues/421)) ([9989590](https://github.com/vymalo/another-agentic-system/commit/9989590137509974869f47c18244cdb381e87e29))

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
