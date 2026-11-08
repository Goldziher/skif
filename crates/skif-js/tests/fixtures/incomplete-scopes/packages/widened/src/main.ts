// A computed import may load any file of the package, so none is judged unused.
export const load = (name: string) => import(name);
