import { useQueryClient } from '@tanstack/react-query';
import { createContext, useContext } from 'react';
import { useNavigate } from 'react-router';
import {
	currentLibraryCache,
	insertLibrary,
	onboardingStore,
	resetOnboardingStore,
	telemetryState,
	unitFormatStore,
	useBridgeMutation,
	useCachedLibraries,
	useMultiZodForm,
	useNormalisedCache,
	useOnboardingStore,
	usePlausibleEvent
} from '@sd/client';
import { RadioGroupField, z } from '@sd/ui';
import { usePlatform } from '~/util/Platform';

import i18n from '../I18n';

export const OnboardingContext = createContext<ReturnType<typeof useContextValue> | null>(null);

// Hook for generating the value to put into `OnboardingContext.Provider`,
// having it separate removes the need for a dedicated context type.
export const useContextValue = () => {
	const libraries = useCachedLibraries();
	const library =
		libraries.data?.find((l) => l.uuid === currentLibraryCache.id) || libraries.data?.[0];

	const form = useFormState();

	return {
		...form,
		libraries,
		library
	};
};

export const shareTelemetry = RadioGroupField.options([
	z.literal('share-telemetry'),
	z.literal('minimal-telemetry')
]).details({
	'share-telemetry': {
		heading: i18n.t('share_anonymous_usage'),
		description: i18n.t('share_anonymous_usage_description')
	},
	'minimal-telemetry': {
		heading: i18n.t('share_bare_minimum'),
		description: i18n.t('share_bare_minimum_description')
	}
});

const schemas = {
	'new-library': z.object({
		name: z.string().min(1, 'Name is required').regex(/[\S]/g).trim()
	}),
	'locations': z.object({
		locations: z.object({
			desktop: z.coerce.boolean(),
			documents: z.coerce.boolean(),
			downloads: z.coerce.boolean(),
			pictures: z.coerce.boolean(),
			music: z.coerce.boolean(),
			videos: z.coerce.boolean()
		})
	}),
	'privacy': z.object({
		shareTelemetry: shareTelemetry.schema
	})
};

const useFormState = () => {
	const obStore = useOnboardingStore();
	const platform = usePlatform();

	const { handleSubmit, ...forms } = useMultiZodForm({
		schemas,
		defaultValues: {
			'new-library': obStore.data?.['new-library'] ?? undefined,
			'locations': obStore.data?.locations ?? { locations: {} },
			'privacy': obStore.data?.privacy ?? {
				shareTelemetry: 'share-telemetry'
			}
		},
		onData: (data) => (onboardingStore.data = { ...obStore.data, ...data })
	});

	const navigate = useNavigate();
	const queryClient = useQueryClient();
	const submitPlausibleEvent = usePlausibleEvent();

	if (window.navigator.language === 'en-US') {
		// not perfect as some linux users use en-US by default, same w/ windows
		unitFormatStore.distanceFormat = 'miles';
		unitFormatStore.temperatureFormat = 'fahrenheit';
	}

	const createLibrary = useBridgeMutation('library.create');
	const cache = useNormalisedCache();

	const submit = handleSubmit(
		async (data) => {
			navigate('./creating-library', { replace: true });

			// opted to place this here as users could change their mind before library creation/onboarding finalization
			// it feels more fitting to configure it here (once)
			telemetryState.shareFullTelemetry = data.privacy.shareTelemetry === 'share-telemetry';

			try {
				// show creation screen for a bit for smoothness
				const [libraryRaw] = await Promise.all([
					createLibrary.mutateAsync({
						name: data['new-library'].name,
						default_locations: data.locations.locations
					}),
					new Promise((res) => setTimeout(res, 500))
				]);
				cache.withNodes(libraryRaw.nodes);
				const library = cache.withCache(libraryRaw.item);
				insertLibrary(queryClient, library);

				platform.refreshMenuBar && platform.refreshMenuBar();

				if (telemetryState.shareFullTelemetry) {
					submitPlausibleEvent({ event: { type: 'libraryCreate' } });
				}

				resetOnboardingStore();
				navigate(`/${library.uuid}`, { replace: true });
			} catch (e) {
				if (e instanceof Error) {
					alert(`Failed to create library. Error: ${e.message}`);
				}
				navigate('./privacy');
			}
		},
		(key) => navigate(`./${key}`)
	);

	return { submit, forms };
};

export const useOnboardingContext = () => {
	const ctx = useContext(OnboardingContext);

	if (!ctx)
		throw new Error('useOnboardingContext must be used within OnboardingContext.Provider');

	return ctx;
};
