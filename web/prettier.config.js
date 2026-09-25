/** @type {import('prettier').Config & import('prettier-plugin-tailwindcss').PluginOptions} */
const config = {
  plugins: ["prettier-plugin-tailwindcss"],
  // Tailwind v4 keeps its theme in CSS; the plugin reads it to sort the site's own colour
  // utilities (text-ink-2, bg-surface, …) like the built-in ones.
  tailwindStylesheet: "./src/styles/globals.css",
};

export default config;
