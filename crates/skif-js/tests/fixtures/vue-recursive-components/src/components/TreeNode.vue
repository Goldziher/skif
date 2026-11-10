<script setup lang="ts">
defineProps<{ children: { id: number; children: unknown[] }[] }>();
</script>

<template>
  <ul>
    <TreeItem v-for="child in children" :key="child.id" :node="child" />
  </ul>
</template>
