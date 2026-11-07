import { ref } from "vue";

export function useCounter() {
  return { count: ref(0) };
}
