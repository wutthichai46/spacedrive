import { RouteObject } from 'react-router';

export default [
	{
		lazy: () => import('../OverviewLayout'),
		children: [
			{ path: 'contacts', lazy: () => import('./contacts') },
			// { path: 'keys', lazy: () => import('./keys') },
			{ path: 'security', lazy: () => import('./security') },
			{ path: 'sharing', lazy: () => import('./sharing') },
			{ path: 'sync', lazy: () => import('./sync') },
			{ path: 'general', lazy: () => import('./general') },
			{ path: 'tags', lazy: () => import('./tags') },
			// { path: 'saved-searches', lazy: () => import('./saved-searches') },
			//this is for edit in tags context menu
			{ path: 'tags/:id', lazy: () => import('./tags') },
			{ path: 'locations', lazy: () => import('./locations') }
		]
	},
	{ path: 'locations/:id', lazy: () => import('./locations/$id') }
] satisfies RouteObject[];
