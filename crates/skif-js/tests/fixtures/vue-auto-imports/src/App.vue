<script setup lang="ts">
const { count } = useCounter();
</script>

<template>
  <HelloCard :count="count" />
</template>
