// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

import { useFeatureIsOn } from '@growthbook/growthbook-react';
import { RpcClientContext } from '@mysten/core';
import { WalletKitProvider } from '@mysten/wallet-kit';
import { ReactQueryDevtools } from '@tanstack/react-query-devtools';
import { PersistQueryClientProvider } from '@tanstack/react-query-persist-client';
import { Fragment, useMemo } from 'react';
import { Toaster } from 'react-hot-toast';
import { Outlet, ScrollRestoration } from 'react-router-dom';

import { usePageView } from '../../hooks/usePageView';
import Footer from '../footer/Footer';
import Header from '../header/Header';

import { NetworkContext, useNetwork } from '~/context';
import { Banner } from '~/ui/Banner';
import { DefaultRpcClient } from '~/utils/api/DefaultRpcClient';
import { queryClient, persister } from '~/utils/queryClient';

export function LayoutContent() {
    const [network, setNetwork] = useNetwork();
    const jsonRpcProvider = useMemo(() => DefaultRpcClient(network), [network]);
    const networkOutage = useFeatureIsOn('explorer-network-outage');

    usePageView();

    return (
        // NOTE: We set a top-level key here to force the entire react tree to be re-created when the network changes:
        <Fragment key={network}>
            <ScrollRestoration />
            <WalletKitProvider
                /*autoConnect={false}*/
                enableUnsafeBurner={import.meta.env.DEV}
            >
                <PersistQueryClientProvider
                    client={queryClient}
                    persistOptions={{ persister }}
                >
                    <RpcClientContext.Provider value={jsonRpcProvider}>
                        <NetworkContext.Provider value={[network, setNetwork]}>
                            <div className="w-full">
                                <Header />
                                <main className="relative z-10 min-h-screen bg-offwhite">
                                    <section className="mx-auto max-w-[1440px] px-5 py-10 2xl:px-0">
                                        {networkOutage && (
                                            <div className="pb-2.5">
                                                <Banner
                                                    variant="warning"
                                                    border
                                                    fullWidth
                                                >
                                                    We&rsquo;re sorry that the
                                                    explorer is running slower
                                                    than usual. We&rsquo;re
                                                    working to fix the issue and
                                                    appreciate your patience.
                                                </Banner>
                                            </div>
                                        )}
                                        <Outlet />
                                    </section>
                                </main>
                                <Footer />
                            </div>

                            <Toaster
                                position="bottom-center"
                                gutter={8}
                                containerStyle={{
                                    top: 40,
                                    left: 40,
                                    bottom: 40,
                                    right: 40,
                                }}
                                toastOptions={{
                                    duration: 4000,
                                    success: {
                                        icon: null,
                                        className:
                                            '!bg-success-light !text-success-dark',
                                    },
                                    error: {
                                        icon: null,
                                        className:
                                            '!bg-issue-light !text-issue-dark',
                                    },
                                }}
                            />
                            <ReactQueryDevtools />
                        </NetworkContext.Provider>
                    </RpcClientContext.Provider>
                </PersistQueryClientProvider>
            </WalletKitProvider>
        </Fragment>
    );
}
