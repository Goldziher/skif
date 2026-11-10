<script setup lang="ts">
defineProps<{ title: string }>();
</script>

<template>
  <section><AppIcon />{{ title }}</section>
</template>
