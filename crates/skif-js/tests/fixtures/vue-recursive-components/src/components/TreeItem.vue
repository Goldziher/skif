<script setup lang="ts">
defineProps<{ node: { id: number; children: unknown[] } }>();
</script>

<template>
  <li>
    {{ node.id }}
    <tree-node v-if="node.children.length" :children="node.children" />
  </li>
</template>
