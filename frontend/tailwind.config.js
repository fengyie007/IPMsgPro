/** @type {import('tailwindcss').Config} */
export default {
  content: ['./index.html', './src/**/*.{js,ts,jsx,tsx}'],
  theme: {
    extend: {
      fontFamily: {
        sans: ['"Microsoft YaHei"', '"PingFang SC"', 'sans-serif'],
      },
      colors: {
        primary: {
          50: '#f0fdf4',
          100: '#dcfce7',
          200: '#bbf7d0',
          300: '#86efac',
          400: '#4ade80',
          500: '#22c55e',
          600: '#16a34a',
          700: '#15803d',
        },
        sidebar: {
          bg: '#2C2C2C',
          hover: '#3C3C3C',
          active: '#07C160',
        },
        chat: {
          bg: '#F5F5F5',
          sent: '#95EC69',
          received: '#FFFFFF',
        },
      },
      width: {
        'sidebar': '60px',
        'user-list': '300px',
      },
    },
  },
  plugins: [],
};
