// Built at run time: its value is not known statically.
export default JSON.parse(process.env.PLAYWRIGHT_BASE ?? '{}');
