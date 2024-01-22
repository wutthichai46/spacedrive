import React from 'react';
import { Text, View } from 'react-native';
import { toggleFeatureFlag, useDebugState, useFeatureFlags } from '@sd/client';
import Card from '~/components/layout/Card';
import { Button } from '~/components/primitive/Button';
import { tw } from '~/lib/tailwind';
import { SettingsStackScreenProps } from '~/navigation/tabs/SettingsStack';

const DebugScreen = ({ navigation }: SettingsStackScreenProps<'Debug'>) => {
	const debugState = useDebugState();
	const featureFlags = useFeatureFlags();

	return (
		<View style={tw`flex-1 p-4`}>
			<Card style={tw`gap-y-4 bg-app-box`}>
				<Text style={tw`font-semibold text-ink`}>Debug</Text>
				<Button onPress={() => (debugState.rspcLogger = !debugState.rspcLogger)}>
					<Text style={tw`text-ink`}>Toggle rspc logger</Text>
				</Button>
				<Text style={tw`text-ink`}>{JSON.stringify(featureFlags)}</Text>
				<Text style={tw`text-ink`}>{JSON.stringify(debugState)}</Text>
				<Button
					onPress={() => {
						navigation.popToTop();
						navigation.replace('Settings');
						debugState.enabled = false;
					}}
				>
					<Text style={tw`text-ink`}>Disable Debug Mode</Text>
				</Button>
			</Card>
		</View>
	);
};

export default DebugScreen;
