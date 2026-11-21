# Changelog

## [0.8.0](https://github.com/vymalo/another-agentic-system/compare/v0.7.1...v0.8.0) (2026-11-21)


### ⚠ BREAKING CHANGES

* **api:** a store outside this repository must implement PromptStore.

### Features

* a person drives a child session: start, send, stop and close (S3 of [#332](https://github.com/vymalo/another-agentic-system/issues/332)) ([#395](https://github.com/vymalo/another-agentic-system/issues/395)) ([eb0bed7](https://github.com/vymalo/another-agentic-system/commit/eb0bed7057f4bd21c346709e7d5de1f32981edd2))
* **a2a:** parse and activate child-sessions/v1 on the agent side (S2c) ([#352](https://github.com/vymalo/another-agentic-system/issues/352)) ([469e1ee](https://github.com/vymalo/another-agentic-system/commit/469e1eed42eca23d6d0fa5942e8841a203fdfea2))
* **agui:** a child session is a subagent and a card in the parent's stream (S2e) ([#356](https://github.com/vymalo/another-agentic-system/issues/356)) ([fe055cb](https://github.com/vymalo/another-agentic-system/commit/fe055cba9e5308e0546532d9c0930e91276ddab4))
* **agui:** a stream ends after a minute and reopens; a message keeps the id its client made ([#427](https://github.com/vymalo/another-agentic-system/issues/427)) ([98d6d61](https://github.com/vymalo/another-agentic-system/commit/98d6d61e29f80efbb53aa4da81608d9a9a89a689))
* **api:** a person searches their own threads (the API, the index, the contract and the mock) ([#322](https://github.com/vymalo/another-agentic-system/issues/322)) ([00a4a30](https://github.com/vymalo/another-agentic-system/commit/00a4a306023da8f0fa14ca4f21387dbf1701f710))
* **api:** a person's prompt library, the API half (ADR 0070) ([#324](https://github.com/vymalo/another-agentic-system/issues/324)) ([bb4d722](https://github.com/vymalo/another-agentic-system/commit/bb4d72275353d9ab166eea924cc9b9e6ff846fd0))
* **app:** follow child sessions and make their threads (S2d) ([#355](https://github.com/vymalo/another-agentic-system/issues/355)) ([a900bc3](https://github.com/vymalo/another-agentic-system/commit/a900bc334f70403f8570c0f39cb2541c4302a398))
* **chart:** a rate limit per client address at the Ingress, off by default ([#307](https://github.com/vymalo/another-agentic-system/issues/307)) ([a34c14c](https://github.com/vymalo/another-agentic-system/commit/a34c14c4756da103c3bebddc626bf34a2881dadf))
* **chart:** three database instances, resources and a transaction pooler (sys[#290](https://github.com/vymalo/another-agentic-system/issues/290), PR 2) ([#310](https://github.com/vymalo/another-agentic-system/issues/310)) ([26d9c17](https://github.com/vymalo/another-agentic-system/commit/26d9c17494696024bfeb2dd4482e314a4e36046c))
* **child-sessions:** child-sessions/v1 contract, ADR 0072, the card read into an offer (S1) ([#347](https://github.com/vymalo/another-agentic-system/issues/347)) ([2654109](https://github.com/vymalo/another-agentic-system/commit/26541095e4d2560f373625800bb7b81fb515d249))
* **core:** child sessions in the core, the parent's three events and the observed round (S2a) ([#349](https://github.com/vymalo/another-agentic-system/issues/349)) ([af1a5f1](https://github.com/vymalo/another-agentic-system/commit/af1a5f1adb8636ea9d1998fba65b612ecd0699d0))
* **dispatcher:** a fair share of the dispatcher for each person (ADR 0066) ([#306](https://github.com/vymalo/another-agentic-system/issues/306)) ([f3ea64f](https://github.com/vymalo/another-agentic-system/commit/f3ea64ff2daf13fefa19990b9313da4380351c7b))
* **orch:** A2UI v1.0 only, a hard cutover (S1 of [#380](https://github.com/vymalo/another-agentic-system/issues/380), ADR 0075) ([#410](https://github.com/vymalo/another-agentic-system/issues/410)) ([ebcee35](https://github.com/vymalo/another-agentic-system/commit/ebcee3509985ba9966d1dc5461c3f61898f9162e))
* **orchestrator:** database.directUrl, so a transaction pooler can carry the rest (sys[#290](https://github.com/vymalo/another-agentic-system/issues/290), PR 1) ([#309](https://github.com/vymalo/another-agentic-system/issues/309)) ([c777d29](https://github.com/vymalo/another-agentic-system/commit/c777d29bfb23f92a208e44180bd530efd0e178fa))
* **orch:** list an agent's own commands from its card (ADR 0079) ([#394](https://github.com/vymalo/another-agentic-system/issues/394)) ([4d7c915](https://github.com/vymalo/another-agentic-system/commit/4d7c915f2d8a10af005fa299b15ee40ad6ca88f7))
* **pg:** a migration runner and a connection that continue what sqlx did ([#425](https://github.com/vymalo/another-agentic-system/issues/425)) ([7fcad12](https://github.com/vymalo/another-agentic-system/commit/7fcad125745961e603a1e8077978092e9c8f22f7))
* **store:** child threads, migration 0027 and the Observe outbox payload (S2b) ([#351](https://github.com/vymalo/another-agentic-system/issues/351)) ([6105fe2](https://github.com/vymalo/another-agentic-system/commit/6105fe29036a8953ebcbd7bba158230707a63408))
* **tools-mcp:** call 2026-07-28 tool servers with requests signed for the thread's owner ([#311](https://github.com/vymalo/another-agentic-system/issues/311)) ([b8b112f](https://github.com/vymalo/another-agentic-system/commit/b8b112f9c3bae68152da9d3777fcb4ab3ef1d4a4))
* **usage:** a usage ledger per person, written in the commit of the event (ADR 0065, PR 1) ([#303](https://github.com/vymalo/another-agentic-system/issues/303)) ([0d50c42](https://github.com/vymalo/another-agentic-system/commit/0d50c4275edc86ee28b1f4c9bb4cf9d2cd730cfa))
* **usage:** price a person's month and answer GET /api/me/usage, with a retention sweep (ADR 0065, PR 2) ([#305](https://github.com/vymalo/another-agentic-system/issues/305)) ([b97de39](https://github.com/vymalo/another-agentic-system/commit/b97de39811ef9a93dffb8ebd467cb0405ec88308))
* **usage:** read and log the sandbox time a task reports in usage/v1 ([#302](https://github.com/vymalo/another-agentic-system/issues/302)) ([6fc8060](https://github.com/vymalo/another-agentic-system/commit/6fc80609f246b5c68f20950d54b4bfc8c492c219))
* **web:** a My tool servers page for the MCP servers a person adds, with a client recipe in the contract (ADR 0046) ([#316](https://github.com/vymalo/another-agentic-system/issues/316)) ([716c260](https://github.com/vymalo/another-agentic-system/commit/716c260afa7047b043c173ddf2d810c9ebed6368))
* **web:** a theme the person chooses, system, light or dark, set before the first paint ([#318](https://github.com/vymalo/another-agentic-system/issues/318)) ([b79132a](https://github.com/vymalo/another-agentic-system/commit/b79132aa9a4507e75a0c81791e16a7227a09f55d))
* **web:** a usage page over GET /api/me/usage (ADR 0065) ([#312](https://github.com/vymalo/another-agentic-system/issues/312)) ([f63fde1](https://github.com/vymalo/another-agentic-system/commit/f63fde1492d879f1af00c5d4c4f9d15e432df518))
* **web:** highlighted code and display math in answers, with a message-text contract for any client ([#314](https://github.com/vymalo/another-agentic-system/issues/314)) ([632384f](https://github.com/vymalo/another-agentic-system/commit/632384fb7f367d36d84eb4196951e638a0699503))
* **web:** quote a message, copy it as Markdown, export a thread as Markdown ([#319](https://github.com/vymalo/another-agentic-system/issues/319)) ([2273c9b](https://github.com/vymalo/another-agentic-system/commit/2273c9be215b797d2a939a04a0c96d8680722add))
* **web:** read an answer aloud, and the keyboard help (ADR 0078) ([#388](https://github.com/vymalo/another-agentic-system/issues/388)) ([3875e07](https://github.com/vymalo/another-agentic-system/commit/3875e0709aba10ceb0f85f774804fca7ccaaa2c7))
* **web:** regenerate the last answer, with a client-neutral recipe in the API ([#313](https://github.com/vymalo/another-agentic-system/issues/313)) ([d5e48f5](https://github.com/vymalo/another-agentic-system/commit/d5e48f590f1519af5a48ab3b79d4e0806cc7148f))
* **web:** search my chats from a box under New chat ([#323](https://github.com/vymalo/another-agentic-system/issues/323)) ([37600d7](https://github.com/vymalo/another-agentic-system/commit/37600d76a867cf09c08195f51e443fc825c31676))
* **web:** subagents in the web: cards, the Subagents tab, the child view (S4 of [#332](https://github.com/vymalo/another-agentic-system/issues/332)) ([#411](https://github.com/vymalo/another-agentic-system/issues/411)) ([71d1fe2](https://github.com/vymalo/another-agentic-system/commit/71d1fe2faf0a6ba841621abc1e10f6aa16fa5d18))
* **web:** the / menu of an agent's own commands (slice 11) ([#398](https://github.com/vymalo/another-agentic-system/issues/398)) ([89762da](https://github.com/vymalo/another-agentic-system/commit/89762da4c7a680870545cd066f38192c41562455))
* **web:** the / menu of prompts and the form that fills one ([#350](https://github.com/vymalo/another-agentic-system/issues/350)) ([9c626db](https://github.com/vymalo/another-agentic-system/commit/9c626dbe1e8c73c717d4886deb73003cdcc11272))
* **web:** the page "My prompts" (ADR 0070, slice P1) ([#325](https://github.com/vymalo/another-agentic-system/issues/325)) ([f367469](https://github.com/vymalo/another-agentic-system/commit/f367469b8ab5b1919c95aafc92ed48e2be4abd53))
* **web:** UI catalog version 7 with confirm, number, scale, date and rank questions, and a contract with conformance vectors ([#317](https://github.com/vymalo/another-agentic-system/issues/317)) ([5ed8950](https://github.com/vymalo/another-agentic-system/commit/5ed8950f09383220cc1bca26c7798b0c7ea135c7))
* **web:** UI catalog version 8: a question that shows what it asks about, and a plan ([#320](https://github.com/vymalo/another-agentic-system/issues/320)) ([d0a732f](https://github.com/vymalo/another-agentic-system/commit/d0a732f21d93485835d7287e9f09a25d2b4fb338))


### Bug Fixes

* **agui:** a surface sent again keeps only its live copy for replay; an asked agent's forms catalog has no show ([#321](https://github.com/vymalo/another-agentic-system/issues/321)) ([5cab8fc](https://github.com/vymalo/another-agentic-system/commit/5cab8fc8216921d0567e1d2bd65a9bdec8d48680))
* **ci:** fail the pooler job where PgBouncer is not ready ([#416](https://github.com/vymalo/another-agentic-system/issues/416)) ([d0b2dec](https://github.com/vymalo/another-agentic-system/commit/d0b2decf38aef10faf4333c3c08a24aa3c3cceb8))
* **deps:** take the advisories of tokio, rustls-webpki, webpki-roots, ctutils and zip ([#300](https://github.com/vymalo/another-agentic-system/issues/300)) ([6224b9c](https://github.com/vymalo/another-agentic-system/commit/6224b9c82f56c28ef1cdd21f2220f454973c6d70))
* **orchestrator:** arm SIGTERM before the address is bound, and make three test suites deterministic ([#304](https://github.com/vymalo/another-agentic-system/issues/304)) ([6739a06](https://github.com/vymalo/another-agentic-system/commit/6739a06ef286474024e38d049ca016e50ede8e62))


### Refactoring

* **dev:** the dev stack without Podman (epic [#359](https://github.com/vymalo/another-agentic-system/issues/359), [#372](https://github.com/vymalo/another-agentic-system/issues/372)) ([#375](https://github.com/vymalo/another-agentic-system/issues/375)) ([3fe1537](https://github.com/vymalo/another-agentic-system/commit/3fe1537f666b9f5e173f4686b1eb645795e90be4))
* **secrets-pg:** off sqlx, onto tokio-postgres ([#382](https://github.com/vymalo/another-agentic-system/issues/382) E1) ([#426](https://github.com/vymalo/another-agentic-system/issues/426)) ([a21bb2e](https://github.com/vymalo/another-agentic-system/commit/a21bb2ee86f9c009d58c32fe24959ef82bd2d720))
* **store-postgres:** off sqlx, onto tokio-postgres ([#382](https://github.com/vymalo/another-agentic-system/issues/382) E1) ([#428](https://github.com/vymalo/another-agentic-system/issues/428)) ([dbff063](https://github.com/vymalo/another-agentic-system/commit/dbff063c7ddc8005b3c6498901ba107c6b3e1ff2))


### Documentation

* **adr:** 0074 agent-team, several primary agents in one chat ([#378](https://github.com/vymalo/another-agentic-system/issues/378)) ([16ff595](https://github.com/vymalo/another-agentic-system/commit/16ff5951266ecbdd0a4ea7417db0e187bd0cd7ea))
* **adr:** 0075, A2UI v1.0 as a hard cutover in R1, and ui-catalog/v2 ([#381](https://github.com/vymalo/another-agentic-system/issues/381)) ([745418a](https://github.com/vymalo/another-agentic-system/commit/745418a9ae78802bddb9d79401532b9a68e0a0f2))
* **adr:** 0076, the orchestrator as a library in an application ([#383](https://github.com/vymalo/another-agentic-system/issues/383)) ([e2ebb09](https://github.com/vymalo/another-agentic-system/commit/e2ebb0950a728656bd4fc46995c992260ff6fd84))
* **adr:** 0077, push notifications for a person's runs ([#385](https://github.com/vymalo/another-agentic-system/issues/385)) ([1b60302](https://github.com/vymalo/another-agentic-system/commit/1b6030212b5806444566448e545f9b759d361021))
* **adr:** 0083 names the vault reference form, and how a rotation ages ([#432](https://github.com/vymalo/another-agentic-system/issues/432)) ([7d1051b](https://github.com/vymalo/another-agentic-system/commit/7d1051ba047e189a840d6d34f8b25a03a99ef53d))
* **adr:** 0083, trusted components — the agent names, the client draws ([#429](https://github.com/vymalo/another-agentic-system/issues/429)) ([5de27b6](https://github.com/vymalo/another-agentic-system/commit/5de27b63259e5b0dde0ccc109521604f5b1a5652))
* **adr:** amend 0074, bounded push, sub-agents, approval expiry ([#379](https://github.com/vymalo/another-agentic-system/issues/379)) ([8fc131f](https://github.com/vymalo/another-agentic-system/commit/8fc131f188da52e97ca12439887df2e8d0fe51f9))
* **adr:** propose ADR 0067, the plan to LibreChat-grade and the client-agnostic rule ([#315](https://github.com/vymalo/another-agentic-system/issues/315)) ([60cb754](https://github.com/vymalo/another-agentic-system/commit/60cb75459d7226be729b8f25c605436bcd333593))
* **api:** ui-catalog/v2, the A2UI v1.0 catalog contract (C0 of [#380](https://github.com/vymalo/another-agentic-system/issues/380)) ([#396](https://github.com/vymalo/another-agentic-system/issues/396)) ([66db4ee](https://github.com/vymalo/another-agentic-system/commit/66db4ee5c885c12f28367a4c623f9c97c38e0930))
* **decisions:** limits per person before a run starts (ADR 0081) ([#424](https://github.com/vymalo/another-agentic-system/issues/424)) ([82e4a33](https://github.com/vymalo/another-agentic-system/commit/82e4a336951850c280871098d3f23d2cf1ca7b8c))
* gVisor is back in milestone M1, and the goal rows are current ([#326](https://github.com/vymalo/another-agentic-system/issues/326)) ([3e3bf12](https://github.com/vymalo/another-agentic-system/commit/3e3bf128594583f3bea9c2d01d7b45d590fe262f))
* payments, pricing and the market plan leave the public roadmap ([#327](https://github.com/vymalo/another-agentic-system/issues/327)) ([50650e6](https://github.com/vymalo/another-agentic-system/commit/50650e6fd1f01ed9b2727ab43bb3136b64cadaa0))
* pgvector for embeddings and vector search, behind a port (ADR 0071) ([#329](https://github.com/vymalo/another-agentic-system/issues/329)) ([291379f](https://github.com/vymalo/another-agentic-system/commit/291379f1156776af3f996026c499fb1843931c9f))
* **plan:** the app-factory board and model selection (ADR 0073) ([#358](https://github.com/vymalo/another-agentic-system/issues/358)) ([78bbe49](https://github.com/vymalo/another-agentic-system/commit/78bbe49bdb8f8a0b9b0b7fe6b0b0d768af412ad9))
* relations beside the vectors, in two plain tables (ADR 0071 amended) ([#330](https://github.com/vymalo/another-agentic-system/issues/330)) ([cf33afa](https://github.com/vymalo/another-agentic-system/commit/cf33afa651c664606bbcaf902272b0524aa09e4e))
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
* two sentences name no head count ([#328](https://github.com/vymalo/another-agentic-system/issues/328)) ([bd204d6](https://github.com/vymalo/another-agentic-system/commit/bd204d6d20fb6ed3772f7d1af33774d382705e9d))


### Tests

* **dev:** child sessions end to end on mocks (S2f) ([#357](https://github.com/vymalo/another-agentic-system/issues/357)) ([118907e](https://github.com/vymalo/another-agentic-system/commit/118907e92e5384593853ffa23de12c765dea78a6))
* **web:** the child session's stream test waits for the session, not the clock ([#419](https://github.com/vymalo/another-agentic-system/issues/419)) ([4a53090](https://github.com/vymalo/another-agentic-system/commit/4a53090eb52efb5a7d29c4dd5d607c0fdc80fbb2))


### Build System

* bump adam-rs to 7418467 (coder sha-7418467) ([#397](https://github.com/vymalo/another-agentic-system/issues/397)) ([868f84a](https://github.com/vymalo/another-agentic-system/commit/868f84a5ef557ec5775ded67647f8255fc3ca93d))
* pin the coder to adam-rs 8501494 (A2UI v1.0 and [#273](https://github.com/vymalo/another-agentic-system/issues/273)) ([#418](https://github.com/vymalo/another-agentic-system/issues/418)) ([9654158](https://github.com/vymalo/another-agentic-system/commit/9654158d458617f4519450e967ffc2a816c2bbbc))


### Chores

* **deploy:** bump orchestrator to sha-00a4a30 ([25f4baf](https://github.com/vymalo/another-agentic-system/commit/25f4bafa667e1d5561fa5e66441ea3cf5e26f368))
* **deploy:** bump orchestrator to sha-0d50c42 ([795ebe8](https://github.com/vymalo/another-agentic-system/commit/795ebe8d6ca1bfa6f42f74ec185b8c9e83068d9a))
* **deploy:** bump orchestrator to sha-118907e ([84ea30f](https://github.com/vymalo/another-agentic-system/commit/84ea30f92df18451bfe8fbe9dd26b63b72b5bd79))
* **deploy:** bump orchestrator to sha-2273c9b ([55bd07f](https://github.com/vymalo/another-agentic-system/commit/55bd07f69fb26f698e0544a6f0cbcc0e04a3ba3a))
* **deploy:** bump orchestrator to sha-2654109 ([17ff793](https://github.com/vymalo/another-agentic-system/commit/17ff7935a8e57eb2cf90a236574b618ab6068eb2))
* **deploy:** bump orchestrator to sha-26d9c17 ([9afaad3](https://github.com/vymalo/another-agentic-system/commit/9afaad3258c098e09794d083a3f3f79821c5450c))
* **deploy:** bump orchestrator to sha-469e1ee ([d1fcf83](https://github.com/vymalo/another-agentic-system/commit/d1fcf83ae9932fc132bb7d68f8af5a371769157e))
* **deploy:** bump orchestrator to sha-4d7c915 ([e7ea2ad](https://github.com/vymalo/another-agentic-system/commit/e7ea2ad223d2d2fd2047f53edc02676e1ae9d450))
* **deploy:** bump orchestrator to sha-5cab8fc ([794050e](https://github.com/vymalo/another-agentic-system/commit/794050e62079b75da13b6919b0dd0ba1338ec8f2))
* **deploy:** bump orchestrator to sha-5ed8950 ([dbeeca1](https://github.com/vymalo/another-agentic-system/commit/dbeeca168c9debcd7371798945bf0c4dea5f5e29))
* **deploy:** bump orchestrator to sha-6105fe2 ([190d1a5](https://github.com/vymalo/another-agentic-system/commit/190d1a55d9ad1774c1b64963253e88690788d7b0))
* **deploy:** bump orchestrator to sha-6224b9c ([15cb544](https://github.com/vymalo/another-agentic-system/commit/15cb5447dc2bd42b3087be0c1bf2a6cb48762e99))
* **deploy:** bump orchestrator to sha-6739a06 ([bbdbb4b](https://github.com/vymalo/another-agentic-system/commit/bbdbb4bfe6ad862fa54f26cd5f7ac2e5d4f19f03))
* **deploy:** bump orchestrator to sha-6fc8060 ([61a9cd6](https://github.com/vymalo/another-agentic-system/commit/61a9cd665b316e3453304a2f3d874c453a70ea2a))
* **deploy:** bump orchestrator to sha-716c260 ([cb3a7f9](https://github.com/vymalo/another-agentic-system/commit/cb3a7f9e7c45a9b76351cc286c61c954cd36332c))
* **deploy:** bump orchestrator to sha-7fcad12 ([cb58255](https://github.com/vymalo/another-agentic-system/commit/cb58255c8e1acbf6fedae72b4dfba9199c71d4a0))
* **deploy:** bump orchestrator to sha-868f84a ([01f51a9](https://github.com/vymalo/another-agentic-system/commit/01f51a96203c3b34d4833bbc8b8265a8908fb1e6))
* **deploy:** bump orchestrator to sha-a21bb2e ([cf6ed05](https://github.com/vymalo/another-agentic-system/commit/cf6ed058bece89b44302040db3cee7e9ed6ebff2))
* **deploy:** bump orchestrator to sha-a900bc3 ([392a156](https://github.com/vymalo/another-agentic-system/commit/392a1569225885d02e59f20e2396ca9baf278c7a))
* **deploy:** bump orchestrator to sha-af1a5f1 ([536577a](https://github.com/vymalo/another-agentic-system/commit/536577ae1c9af6df4f19884038b3f9b7a3482738))
* **deploy:** bump orchestrator to sha-b8b112f ([c1b2593](https://github.com/vymalo/another-agentic-system/commit/c1b2593f833863465eea6c12f6ef146370033817))
* **deploy:** bump orchestrator to sha-b97de39 ([ad2326f](https://github.com/vymalo/another-agentic-system/commit/ad2326f5508b61839f2f8994d1277d6437efd69d))
* **deploy:** bump orchestrator to sha-bb4d722 ([438509a](https://github.com/vymalo/another-agentic-system/commit/438509a6d044593170b78115d943dafcc9f7e0d9))
* **deploy:** bump orchestrator to sha-c777d29 ([ab49f87](https://github.com/vymalo/another-agentic-system/commit/ab49f877494a853af2efd4b217fbe23e0b0ef665))
* **deploy:** bump orchestrator to sha-d0a732f ([a548f06](https://github.com/vymalo/another-agentic-system/commit/a548f0623b605e0a6a202ddb5ad1db3fcaee645e))
* **deploy:** bump orchestrator to sha-d0b2dec ([0fc8a0a](https://github.com/vymalo/another-agentic-system/commit/0fc8a0ad99a8631e4d9f7713df0adba9ac35ee9b))
* **deploy:** bump orchestrator to sha-d5e48f5 ([d416a96](https://github.com/vymalo/another-agentic-system/commit/d416a9601cb3f691b71d444f5364b27f99b40bdc))
* **deploy:** bump orchestrator to sha-eb0bed7 ([04df012](https://github.com/vymalo/another-agentic-system/commit/04df01281146b7e15e43888da478d2d7a13dcb42))
* **deploy:** bump orchestrator to sha-ebcee35 ([52747ea](https://github.com/vymalo/another-agentic-system/commit/52747ea795703768b9de3fe2ae3a3092bef8eb87))
* **deploy:** bump orchestrator to sha-f3ea64f ([c03035c](https://github.com/vymalo/another-agentic-system/commit/c03035cedf5ccafd3160526636035302ea34ad2b))
* **deploy:** bump orchestrator to sha-fe055cb ([ae4b54d](https://github.com/vymalo/another-agentic-system/commit/ae4b54d94c0c2c144e529556619b5127d8d01689))
* **deploy:** bump web to sha-00a4a30 ([cd70908](https://github.com/vymalo/another-agentic-system/commit/cd70908f54bcd740fdf96906037cfd188a47adf3))
* **deploy:** bump web to sha-2273c9b ([97876ff](https://github.com/vymalo/another-agentic-system/commit/97876ffbf81700a389da20ff39e3185d70859507))
* **deploy:** bump web to sha-37600d7 ([d70436d](https://github.com/vymalo/another-agentic-system/commit/d70436d361317b0edb91be4ab04aa03f96e754ff))
* **deploy:** bump web to sha-3875e07 ([0c96dcf](https://github.com/vymalo/another-agentic-system/commit/0c96dcfaad2bd102cc4e17e3fb7e5c41fdd7ef00))
* **deploy:** bump web to sha-4a53090 ([34c9e29](https://github.com/vymalo/another-agentic-system/commit/34c9e291284126d8ac999e3a39cfe9de8b0ab89b))
* **deploy:** bump web to sha-4d7c915 ([306da5b](https://github.com/vymalo/another-agentic-system/commit/306da5ba3a1f2974511c86d05ca8d59b7bf15f65))
* **deploy:** bump web to sha-5cab8fc ([36a9abb](https://github.com/vymalo/another-agentic-system/commit/36a9abbc2a3c9c04e9fbab365a0f2017f6dae592))
* **deploy:** bump web to sha-5ed8950 ([5b8f009](https://github.com/vymalo/another-agentic-system/commit/5b8f0090256c0017d0042d6e1c8a6d28990012fd))
* **deploy:** bump web to sha-632384f ([542505c](https://github.com/vymalo/another-agentic-system/commit/542505cc975616f883d312e7cda6ce4b57493614))
* **deploy:** bump web to sha-6fc8060 ([6b1ab65](https://github.com/vymalo/another-agentic-system/commit/6b1ab65c79f4367180ea5ad0aabf16212ee3eb3e))
* **deploy:** bump web to sha-716c260 ([fafb16d](https://github.com/vymalo/another-agentic-system/commit/fafb16dd90b9a2bce78961f65949eff441349683))
* **deploy:** bump web to sha-71d1fe2 ([8fa5d75](https://github.com/vymalo/another-agentic-system/commit/8fa5d757cfd7a1a5ddfb1fd12610efc4d81c56a8))
* **deploy:** bump web to sha-89762da ([5f23c59](https://github.com/vymalo/another-agentic-system/commit/5f23c59fa16bcdb48cb67ce1cc66085c1295c00e))
* **deploy:** bump web to sha-9c626db ([3212875](https://github.com/vymalo/another-agentic-system/commit/3212875e0253768884a9bd17b7848e18a9a466df))
* **deploy:** bump web to sha-a900bc3 ([e98be7f](https://github.com/vymalo/another-agentic-system/commit/e98be7fb05bc2228d0cbda697aad7853f35c6e8c))
* **deploy:** bump web to sha-af1a5f1 ([3761d29](https://github.com/vymalo/another-agentic-system/commit/3761d29044b4ee00e7c9527ef11fc06c7491a30c))
* **deploy:** bump web to sha-af95948 ([9dc6907](https://github.com/vymalo/another-agentic-system/commit/9dc6907a75b0b98ebf4f663d277d8973d03fa639))
* **deploy:** bump web to sha-b79132a ([d278240](https://github.com/vymalo/another-agentic-system/commit/d278240cfdf03d3dc2f3f014897392260a273999))
* **deploy:** bump web to sha-b97de39 ([e978437](https://github.com/vymalo/another-agentic-system/commit/e97843743226fe404c60262ec09c8624feb55e3b))
* **deploy:** bump web to sha-bb4d722 ([f0a2745](https://github.com/vymalo/another-agentic-system/commit/f0a2745bff7c57d52143b289fcb3ec565fb656c7))
* **deploy:** bump web to sha-d0a732f ([c860609](https://github.com/vymalo/another-agentic-system/commit/c860609c5a7862b46f91a86b7345b141b3b19e73))
* **deploy:** bump web to sha-d5e48f5 ([59c5ce5](https://github.com/vymalo/another-agentic-system/commit/59c5ce53d8527c546c0003f7ae8493cdf4f7b82d))
* **deploy:** bump web to sha-eb0bed7 ([eaecb7b](https://github.com/vymalo/another-agentic-system/commit/eaecb7bd00dd87da04194b2fcd5101402c72502b))
* **deploy:** bump web to sha-f367469 ([2527b34](https://github.com/vymalo/another-agentic-system/commit/2527b34d5ca52516b7accff4692892bce1515d4d))
* **deploy:** bump web to sha-f63fde1 ([7578dfc](https://github.com/vymalo/another-agentic-system/commit/7578dfc907b01b5db21dc2b0a97c70c536275586))
* **deploy:** bump web to sha-fe055cb ([ce1c62f](https://github.com/vymalo/another-agentic-system/commit/ce1c62fba28da09ad4f2a8011b8b3823aa4f5ad6))
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
