import { useNavigation, useRoute } from '@react-navigation/native';
import { AppLogo, BloomOne } from '@sd/assets/images';
import { sdintro } from '@sd/assets/videos';
import { ResizeMode, Video } from 'expo-av';
import { MotiView } from 'moti';
import { CaretLeft } from 'phosphor-react-native';
import { useEffect } from 'react';
import { Image, KeyboardAvoidingView, Platform, Pressable, Text, View } from 'react-native';
import Animated from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { useOnboardingStore } from '@sd/client';
import { FadeInUpAnimation, LogoAnimation } from '~/components/animation/layout';
import { AnimatedButton } from '~/components/primitive/Button';
import { styled, tw, twStyle } from '~/lib/tailwind';
import { OnboardingStackScreenProps } from '~/navigation/OnboardingNavigator';

export function OnboardingContainer({ children }: React.PropsWithChildren) {
	const navigation = useNavigation();
	const route = useRoute();
	const { top, bottom } = useSafeAreaInsets();
	const store = useOnboardingStore();
	return (
		<View style={tw`relative flex-1`}>
			{store.showIntro && (
				<View
					style={twStyle(
						'absolute z-50 mx-auto h-full w-full flex-1 items-center justify-center',
						Platform.OS === 'ios' ? 'bg-[#1C1E27]' : 'bg-[#1E1D28]'
					)}
				>
					<Video
						style={tw`w-[700px] h-[700px]`}
						shouldPlay
						onPlaybackStatusUpdate={(status) => {
							if (status.isLoaded && status.didJustFinish) {
								store.showIntro = false;
							}
						}}
						source={sdintro}
						isMuted
						resizeMode={ResizeMode.CONTAIN}
					/>
				</View>
			)}
			{route.name !== 'GetStarted' && route.name !== 'CreatingLibrary' && (
				<Pressable
					style={twStyle('absolute left-6 z-50', { top: top + 16 })}
					onPress={() => navigation.goBack()}
				>
					<CaretLeft size={24} weight="bold" color="white" />
				</Pressable>
			)}
			<View style={tw`z-10 items-center justify-center flex-1`}>
				<KeyboardAvoidingView
					behavior={Platform.OS === 'ios' ? 'padding' : 'height'}
					keyboardVerticalOffset={bottom}
					style={tw`items-center justify-center flex-1 w-full`}
				>
					<MotiView style={tw`items-center justify-center w-full px-4`}>
						{children}
					</MotiView>
				</KeyboardAvoidingView>
				<Text style={tw`absolute text-xs bottom-8 text-ink-dull/50`}>
					&copy; {new Date().getFullYear()} Spacedrive Technology Inc.
				</Text>
			</View>
			{/* Bloom */}
			<Image source={BloomOne} style={tw`absolute w-screen h-screen top-100 opacity-20`} />
		</View>
	);
}

export const OnboardingTitle = styled(
	Animated.Text,
	'text-ink text-center text-4xl font-extrabold leading-tight'
);

export const OnboardingDescription = styled(
	Text,
	'text-ink-dull text-center text-base leading-relaxed'
);

const GetStartedScreen = ({ navigation }: OnboardingStackScreenProps<'GetStarted'>) => {
	//initial render - reset video intro value
	const store = useOnboardingStore();
	useEffect(() => {
		store.showIntro = true;
	}, []);
	return (
		<OnboardingContainer>
			{/* Logo */}
			<LogoAnimation style={tw`items-center`}>
				<Image source={AppLogo} style={tw`h-30 w-30`} />
			</LogoAnimation>
			{/* Title */}
			<FadeInUpAnimation delay={500} style={tw`mt-8`}>
				<OnboardingTitle>The file explorer from the future.</OnboardingTitle>
			</FadeInUpAnimation>
			{/* Description */}
			<FadeInUpAnimation delay={800} style={tw`mt-8`}>
				<OnboardingDescription style={tw`px-4`}>
					Welcome to Spacedrive, an open source cross-platform file manager.
				</OnboardingDescription>
			</FadeInUpAnimation>
			{/* Get Started Button */}
			<FadeInUpAnimation delay={1200} style={tw`mt-8`}>
				<AnimatedButton variant="accent" onPress={() => navigation.push('NewLibrary')}>
					<Text style={tw`text-base font-medium text-center text-ink`}>Get Started</Text>
				</AnimatedButton>
			</FadeInUpAnimation>
		</OnboardingContainer>
	);
};

export default GetStartedScreen;
