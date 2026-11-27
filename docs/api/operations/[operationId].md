---
aside: false
outline: false
title: Chat API operation
---

<script setup>
import { useRoute } from "vitepress";

const operationId = useRoute().data.params.operationId;
</script>

<OAOperation :operationId="operationId" />
