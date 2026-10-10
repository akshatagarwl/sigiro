import { eslintCompatPlugin } from "@oxlint/plugins";

export default eslintCompatPlugin({
  meta: { name: "policy" },
  rules: {
    "no-comments": {
      create(context) {
        return {
          Program() {
            for (const comment of context.sourceCode.getAllComments()) {
              context.report({
                loc: comment.loc,
                message: "Code comments are not allowed.",
              });
            }
          },
        };
      },
    },
    "no-synthetic-tests": {
      create(context) {
        return {
          Program(node) {
            if (
              /(^|\/)__(tests|mocks)__\/|(^|\/)(unit|test|tests|smoke|spec|mock|mocks)(\/|\.)|[.-](unit|test|spec|smoke|mock)\.[cm]?[jt]sx?$/i.test(
                context.filename
              )
            ) {
              context.report({
                node,
                message:
                  "Unit, smoke, and mock files are not allowed; verify real runs.",
              });
            }
          },
          ImportDeclaration(node) {
            if (/^(bun:test|node:test|vitest|jest)$/.test(node.source.value)) {
              context.report({
                node,
                message:
                  "Synthetic test frameworks are not allowed; verify real runs.",
              });
            }
          },
          CallExpression(node) {
            if (
              node.callee.type === "Identifier" &&
              /^(describe|it|test)$/.test(node.callee.name)
            ) {
              context.report({
                node,
                message:
                  "Unit and smoke test cases are not allowed; verify real runs.",
              });
            }
          },
        };
      },
    },
  },
});
