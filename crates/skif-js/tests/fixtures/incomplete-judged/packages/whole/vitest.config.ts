// Unknown as a whole: the config may name any file of the package, so the package stays unjudged.
export default JSON.parse(process.env.VITEST_CONFIG ?? '{}');
