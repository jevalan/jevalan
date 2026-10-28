import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'https://jevalan.com',
  integrations: [
    starlight({
      title: 'Jevalan',
      description: 'Proxy local de decisiones para agentes y herramientas.',
      locales: {
        root: { label: 'Español', lang: 'es' },
        en: { label: 'English' },
        'zh-cn': { label: '简体中文', lang: 'zh-CN' },
      },
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/jevalan/jevalan' }],
      sidebar: [
        {
          label: 'Guía de uso',
          translations: { en: 'User guide', 'zh-CN': '使用指南' },
          items: ['primeros-pasos', 'ayuda', 'conceptos', 'arquitectura'],
        },
        {
          label: 'Construir decisiones',
          translations: { en: 'Build decisions', 'zh-CN': '构建决策' },
          items: ['flujos', 'capacidades', 'api'],
        },
        {
          label: 'Configurar y operar',
          translations: { en: 'Configure and operate', 'zh-CN': '配置与运维' },
          items: ['configuracion', 'comandos', 'integraciones', 'laboratorio', 'solucion-de-problemas'],
        },
      ],
      customCss: ['./src/styles/custom.css'],
      editLink: {
        baseUrl: 'https://github.com/jevalan/jevalan/edit/main/docs/site/src/content/docs/',
      },
    }),
  ],
});
