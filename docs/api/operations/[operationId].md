---
aside: false
outline: false
title: Chat API operation
---

<script setup>
import { useRoute } from "vitepress";
import { useChatApiSpec } from "../../.vitepress/openapi/client";

useChatApiSpec();
const operationId = useRoute().data.params.operationId;
</script>

<OAOperation :operationId="operationId" />
