import AsyncStorage from '@react-native-async-storage/async-storage';
import { useBottomTabBarHeight } from '@react-navigation/bottom-tabs';
import { CheckCircle } from 'phosphor-react-native';
import React from 'react';
import { ScrollView, View } from 'react-native';
import { resetOnboardingStore, useLibraryQuery } from '@sd/client';
import { PulseAnimation } from '~/components/animation/lottie';
import BrowseLocations from '~/components/browse/BrowseLocations';
import BrowseTags from '~/components/browse/BrowseTags';
import Categories from '~/components/browse/Categories';
import Jobs from '~/components/browse/Jobs';
import { tw, twStyle } from '~/lib/tailwind';

function JobIcon() {
	const { data: isActive } = useLibraryQuery(['jobs.isActive']);
	return isActive ? (
		<PulseAnimation style={tw`h-[24px] w-[32px]`} speed={1.5} />
	) : (
		<CheckCircle color="white" size={24} />
	);
}

export default function BrowseScreen() {
	const height = useBottomTabBarHeight();
	return (
		<ScrollView style={twStyle('flex-1 bg-mobile-screen', { marginBottom: height })}>
			<View style={twStyle('justify-between gap-6 py-5')}>
				{/*Categories*/}
				<Categories />
				{/* Locations */}
				<BrowseLocations />
				{/* Tags */}
				<BrowseTags />
				{/* Jobs */}
				<Jobs />
				{/* <View style={tw`flex-row items-center w-full gap-x-4`}>
					<JobManagerContextProvider>
						<Pressable onPress={() => modalRef.current?.present()}>
							<JobIcon />
						</Pressable>
						<JobManagerModal ref={modalRef} />
					</JobManagerContextProvider>
				</View> */}
			</View>
		</ScrollView>
	);
}
