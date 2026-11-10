<script setup lang="ts">
import { h, resolveComponent } from "vue";

const label = "Save";
const dialog = () => h(resolveComponent("AppDialog"));
</script>

<template>
  <AppButton :label="label" />
  <app-card title="Card" />
  <myField />
  <component is="AppBadge" />
  <component :is="dialog" />
  <button>native</button>
</template>
