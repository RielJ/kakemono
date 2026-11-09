/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  theme: {
    extend: {
      colors: {
        // macOS accent colours — used in Sidebar, toggle etc.
        accent:      '#007AFF',
        'accent-dark': '#0A84FF',
      },
    },
  },
  plugins: [],
}
