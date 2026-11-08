// Built at run time: its value is not known statically.
export default makeBase();

function makeBase() {
  return JSON.parse(process.env.VITE_BASE ?? '{}');
}
