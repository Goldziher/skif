<script setup lang="ts">
const tree = [{ id: 1, children: [] }];
</script>

<template>
  <TreeNode :children="tree" />
</template>
