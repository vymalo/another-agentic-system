---
aside: false
outline: false
title: Chat API tag
---

<script setup>
import { useRoute } from "vitepress";

const tag = useRoute().data.params.tag;
</script>

<OASpec :tags="[tag]" hide-info hide-servers />
