// @generated
/// Generated client implementations.
pub mod dashboard_service_client {
    #![allow(
        unused_variables,
        dead_code,
        missing_docs,
        clippy::wildcard_imports,
        clippy::let_unit_value,
    )]
    use tonic::codegen::*;
    use tonic::codegen::http::Uri;
    ///
    #[derive(Debug, Clone)]
    pub struct DashboardServiceClient<T> {
        inner: tonic::client::Grpc<T>,
    }
    impl DashboardServiceClient<tonic::transport::Channel> {
        /// Attempt to create a new client by connecting to a given endpoint.
        pub async fn connect<D>(dst: D) -> Result<Self, tonic::transport::Error>
        where
            D: TryInto<tonic::transport::Endpoint>,
            D::Error: Into<StdError>,
        {
            let conn = tonic::transport::Endpoint::new(dst)?.connect().await?;
            Ok(Self::new(conn))
        }
    }
    impl<T> DashboardServiceClient<T>
    where
        T: tonic::client::GrpcService<tonic::body::Body>,
        T::Error: Into<StdError>,
        T::ResponseBody: Body<Data = Bytes> + std::marker::Send + 'static,
        <T::ResponseBody as Body>::Error: Into<StdError> + std::marker::Send,
    {
        pub fn new(inner: T) -> Self {
            let inner = tonic::client::Grpc::new(inner);
            Self { inner }
        }
        pub fn with_origin(inner: T, origin: Uri) -> Self {
            let inner = tonic::client::Grpc::with_origin(inner, origin);
            Self { inner }
        }
        pub fn with_interceptor<F>(
            inner: T,
            interceptor: F,
        ) -> DashboardServiceClient<InterceptedService<T, F>>
        where
            F: tonic::service::Interceptor,
            T::ResponseBody: Default,
            T: tonic::codegen::Service<
                http::Request<tonic::body::Body>,
                Response = http::Response<
                    <T as tonic::client::GrpcService<tonic::body::Body>>::ResponseBody,
                >,
            >,
            <T as tonic::codegen::Service<
                http::Request<tonic::body::Body>,
            >>::Error: Into<StdError> + std::marker::Send + std::marker::Sync,
        {
            DashboardServiceClient::new(InterceptedService::new(inner, interceptor))
        }
        /// Compress requests with the given encoding.
        ///
        /// This requires the server to support it otherwise it might respond with an
        /// error.
        #[must_use]
        pub fn send_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.inner = self.inner.send_compressed(encoding);
            self
        }
        /// Enable decompressing responses.
        #[must_use]
        pub fn accept_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.inner = self.inner.accept_compressed(encoding);
            self
        }
        /// Limits the maximum size of a decoded message.
        ///
        /// Default: `4MB`
        #[must_use]
        pub fn max_decoding_message_size(mut self, limit: usize) -> Self {
            self.inner = self.inner.max_decoding_message_size(limit);
            self
        }
        /// Limits the maximum size of an encoded message.
        ///
        /// Default: `usize::MAX`
        #[must_use]
        pub fn max_encoding_message_size(mut self, limit: usize) -> Self {
            self.inner = self.inner.max_encoding_message_size(limit);
            self
        }
        ///
        pub async fn get_all_hosts(
            &mut self,
            request: impl tonic::IntoRequest<super::GetAllHostsRequest>,
        ) -> std::result::Result<
            tonic::Response<super::GetHostsResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/GetAllHosts",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("dashboard.v1.DashboardService", "GetAllHosts"));
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn update_host_name(
            &mut self,
            request: impl tonic::IntoRequest<super::UpdateNameRequest>,
        ) -> std::result::Result<tonic::Response<super::Host>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/UpdateHostName",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new("dashboard.v1.DashboardService", "UpdateHostName"),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn delete_host(
            &mut self,
            request: impl tonic::IntoRequest<super::Id>,
        ) -> std::result::Result<tonic::Response<()>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/DeleteHost",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("dashboard.v1.DashboardService", "DeleteHost"));
            self.inner.unary(req, path, codec).await
        }
        /** sends updates on the host connection status. On first connect receives the current state will be send
 as a bunch of connected = true messages
*/
        pub async fn connection_state(
            &mut self,
            request: impl tonic::IntoRequest<()>,
        ) -> std::result::Result<
            tonic::Response<tonic::codec::Streaming<super::HostConnectionEvent>>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/ConnectionState",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new("dashboard.v1.DashboardService", "ConnectionState"),
                );
            self.inner.server_streaming(req, path, codec).await
        }
        ///
        pub async fn deploy(
            &mut self,
            request: impl tonic::IntoRequest<super::DeployHostRequest>,
        ) -> std::result::Result<tonic::Response<super::Task>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/Deploy",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("dashboard.v1.DashboardService", "Deploy"));
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn multicast(
            &mut self,
            request: impl tonic::IntoRequest<super::MulticastHostsRequest>,
        ) -> std::result::Result<tonic::Response<()>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/Multicast",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("dashboard.v1.DashboardService", "Multicast"));
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn reboot(
            &mut self,
            request: impl tonic::IntoRequest<super::RebootHostsRequest>,
        ) -> std::result::Result<tonic::Response<()>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/Reboot",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("dashboard.v1.DashboardService", "Reboot"));
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn wake_on_lan(
            &mut self,
            request: impl tonic::IntoRequest<super::WakeOnLanRequest>,
        ) -> std::result::Result<tonic::Response<()>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/WakeOnLan",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("dashboard.v1.DashboardService", "WakeOnLan"));
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn get_all_images(
            &mut self,
            request: impl tonic::IntoRequest<()>,
        ) -> std::result::Result<
            tonic::Response<super::GetImagesResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/GetAllImages",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new("dashboard.v1.DashboardService", "GetAllImages"),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn update_image_name(
            &mut self,
            request: impl tonic::IntoRequest<super::UpdateNameRequest>,
        ) -> std::result::Result<tonic::Response<super::Image>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/UpdateImageName",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new("dashboard.v1.DashboardService", "UpdateImageName"),
                );
            self.inner.unary(req, path, codec).await
        }
        /** Creates an image and captures it from the specified host.
 If this host is not online, there will be a task put in the queue
*/
        pub async fn create_image(
            &mut self,
            request: impl tonic::IntoRequest<super::CreateImageRequest>,
        ) -> std::result::Result<tonic::Response<super::Image>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/CreateImage",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("dashboard.v1.DashboardService", "CreateImage"));
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn delete_image(
            &mut self,
            request: impl tonic::IntoRequest<super::Id>,
        ) -> std::result::Result<tonic::Response<()>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/DeleteImage",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("dashboard.v1.DashboardService", "DeleteImage"));
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn get_all_tasks(
            &mut self,
            request: impl tonic::IntoRequest<()>,
        ) -> std::result::Result<
            tonic::Response<super::GetTasksResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/GetAllTasks",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("dashboard.v1.DashboardService", "GetAllTasks"));
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn cancel_task(
            &mut self,
            request: impl tonic::IntoRequest<super::Id>,
        ) -> std::result::Result<tonic::Response<()>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/CancelTask",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("dashboard.v1.DashboardService", "CancelTask"));
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn retry_task(
            &mut self,
            request: impl tonic::IntoRequest<super::Id>,
        ) -> std::result::Result<tonic::Response<()>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/RetryTask",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("dashboard.v1.DashboardService", "RetryTask"));
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn create_group(
            &mut self,
            request: impl tonic::IntoRequest<super::CreateGroupRequest>,
        ) -> std::result::Result<tonic::Response<super::Group>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/CreateGroup",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("dashboard.v1.DashboardService", "CreateGroup"));
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn update_group_name(
            &mut self,
            request: impl tonic::IntoRequest<super::UpdateNameRequest>,
        ) -> std::result::Result<tonic::Response<super::Group>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/UpdateGroupName",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new("dashboard.v1.DashboardService", "UpdateGroupName"),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn get_all_groups(
            &mut self,
            request: impl tonic::IntoRequest<()>,
        ) -> std::result::Result<
            tonic::Response<super::GetGroupsResponse>,
            tonic::Status,
        > {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/GetAllGroups",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new("dashboard.v1.DashboardService", "GetAllGroups"),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn update_group_memberships(
            &mut self,
            request: impl tonic::IntoRequest<super::UpdateGroupRequest>,
        ) -> std::result::Result<tonic::Response<super::Group>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/UpdateGroupMemberships",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(
                    GrpcMethod::new(
                        "dashboard.v1.DashboardService",
                        "UpdateGroupMemberships",
                    ),
                );
            self.inner.unary(req, path, codec).await
        }
        ///
        pub async fn delete_group(
            &mut self,
            request: impl tonic::IntoRequest<super::Id>,
        ) -> std::result::Result<tonic::Response<()>, tonic::Status> {
            self.inner
                .ready()
                .await
                .map_err(|e| {
                    tonic::Status::unknown(
                        format!("Service was not ready: {}", e.into()),
                    )
                })?;
            let codec = tonic_prost::ProstCodec::default();
            let path = http::uri::PathAndQuery::from_static(
                "/dashboard.v1.DashboardService/DeleteGroup",
            );
            let mut req = request.into_request();
            req.extensions_mut()
                .insert(GrpcMethod::new("dashboard.v1.DashboardService", "DeleteGroup"));
            self.inner.unary(req, path, codec).await
        }
    }
}
/// Generated server implementations.
pub mod dashboard_service_server {
    #![allow(
        unused_variables,
        dead_code,
        missing_docs,
        clippy::wildcard_imports,
        clippy::let_unit_value,
    )]
    use tonic::codegen::*;
    /// Generated trait containing gRPC methods that should be implemented for use with DashboardServiceServer.
    #[async_trait]
    pub trait DashboardService: std::marker::Send + std::marker::Sync + 'static {
        ///
        async fn get_all_hosts(
            &self,
            request: tonic::Request<super::GetAllHostsRequest>,
        ) -> std::result::Result<
            tonic::Response<super::GetHostsResponse>,
            tonic::Status,
        >;
        ///
        async fn update_host_name(
            &self,
            request: tonic::Request<super::UpdateNameRequest>,
        ) -> std::result::Result<tonic::Response<super::Host>, tonic::Status>;
        ///
        async fn delete_host(
            &self,
            request: tonic::Request<super::Id>,
        ) -> std::result::Result<tonic::Response<()>, tonic::Status>;
        /// Server streaming response type for the ConnectionState method.
        type ConnectionStateStream: tonic::codegen::tokio_stream::Stream<
                Item = std::result::Result<super::HostConnectionEvent, tonic::Status>,
            >
            + std::marker::Send
            + 'static;
        /** sends updates on the host connection status. On first connect receives the current state will be send
 as a bunch of connected = true messages
*/
        async fn connection_state(
            &self,
            request: tonic::Request<()>,
        ) -> std::result::Result<
            tonic::Response<Self::ConnectionStateStream>,
            tonic::Status,
        >;
        ///
        async fn deploy(
            &self,
            request: tonic::Request<super::DeployHostRequest>,
        ) -> std::result::Result<tonic::Response<super::Task>, tonic::Status>;
        ///
        async fn multicast(
            &self,
            request: tonic::Request<super::MulticastHostsRequest>,
        ) -> std::result::Result<tonic::Response<()>, tonic::Status>;
        ///
        async fn reboot(
            &self,
            request: tonic::Request<super::RebootHostsRequest>,
        ) -> std::result::Result<tonic::Response<()>, tonic::Status>;
        ///
        async fn wake_on_lan(
            &self,
            request: tonic::Request<super::WakeOnLanRequest>,
        ) -> std::result::Result<tonic::Response<()>, tonic::Status>;
        ///
        async fn get_all_images(
            &self,
            request: tonic::Request<()>,
        ) -> std::result::Result<
            tonic::Response<super::GetImagesResponse>,
            tonic::Status,
        >;
        ///
        async fn update_image_name(
            &self,
            request: tonic::Request<super::UpdateNameRequest>,
        ) -> std::result::Result<tonic::Response<super::Image>, tonic::Status>;
        /** Creates an image and captures it from the specified host.
 If this host is not online, there will be a task put in the queue
*/
        async fn create_image(
            &self,
            request: tonic::Request<super::CreateImageRequest>,
        ) -> std::result::Result<tonic::Response<super::Image>, tonic::Status>;
        ///
        async fn delete_image(
            &self,
            request: tonic::Request<super::Id>,
        ) -> std::result::Result<tonic::Response<()>, tonic::Status>;
        ///
        async fn get_all_tasks(
            &self,
            request: tonic::Request<()>,
        ) -> std::result::Result<
            tonic::Response<super::GetTasksResponse>,
            tonic::Status,
        >;
        ///
        async fn cancel_task(
            &self,
            request: tonic::Request<super::Id>,
        ) -> std::result::Result<tonic::Response<()>, tonic::Status>;
        ///
        async fn retry_task(
            &self,
            request: tonic::Request<super::Id>,
        ) -> std::result::Result<tonic::Response<()>, tonic::Status>;
        ///
        async fn create_group(
            &self,
            request: tonic::Request<super::CreateGroupRequest>,
        ) -> std::result::Result<tonic::Response<super::Group>, tonic::Status>;
        ///
        async fn update_group_name(
            &self,
            request: tonic::Request<super::UpdateNameRequest>,
        ) -> std::result::Result<tonic::Response<super::Group>, tonic::Status>;
        ///
        async fn get_all_groups(
            &self,
            request: tonic::Request<()>,
        ) -> std::result::Result<
            tonic::Response<super::GetGroupsResponse>,
            tonic::Status,
        >;
        ///
        async fn update_group_memberships(
            &self,
            request: tonic::Request<super::UpdateGroupRequest>,
        ) -> std::result::Result<tonic::Response<super::Group>, tonic::Status>;
        ///
        async fn delete_group(
            &self,
            request: tonic::Request<super::Id>,
        ) -> std::result::Result<tonic::Response<()>, tonic::Status>;
    }
    ///
    #[derive(Debug)]
    pub struct DashboardServiceServer<T> {
        inner: Arc<T>,
        accept_compression_encodings: EnabledCompressionEncodings,
        send_compression_encodings: EnabledCompressionEncodings,
        max_decoding_message_size: Option<usize>,
        max_encoding_message_size: Option<usize>,
    }
    impl<T> DashboardServiceServer<T> {
        pub fn new(inner: T) -> Self {
            Self::from_arc(Arc::new(inner))
        }
        pub fn from_arc(inner: Arc<T>) -> Self {
            Self {
                inner,
                accept_compression_encodings: Default::default(),
                send_compression_encodings: Default::default(),
                max_decoding_message_size: None,
                max_encoding_message_size: None,
            }
        }
        pub fn with_interceptor<F>(
            inner: T,
            interceptor: F,
        ) -> InterceptedService<Self, F>
        where
            F: tonic::service::Interceptor,
        {
            InterceptedService::new(Self::new(inner), interceptor)
        }
        /// Enable decompressing requests with the given encoding.
        #[must_use]
        pub fn accept_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.accept_compression_encodings.enable(encoding);
            self
        }
        /// Compress responses with the given encoding, if the client supports it.
        #[must_use]
        pub fn send_compressed(mut self, encoding: CompressionEncoding) -> Self {
            self.send_compression_encodings.enable(encoding);
            self
        }
        /// Limits the maximum size of a decoded message.
        ///
        /// Default: `4MB`
        #[must_use]
        pub fn max_decoding_message_size(mut self, limit: usize) -> Self {
            self.max_decoding_message_size = Some(limit);
            self
        }
        /// Limits the maximum size of an encoded message.
        ///
        /// Default: `usize::MAX`
        #[must_use]
        pub fn max_encoding_message_size(mut self, limit: usize) -> Self {
            self.max_encoding_message_size = Some(limit);
            self
        }
    }
    impl<T, B> tonic::codegen::Service<http::Request<B>> for DashboardServiceServer<T>
    where
        T: DashboardService,
        B: Body + std::marker::Send + 'static,
        B::Error: Into<StdError> + std::marker::Send + 'static,
    {
        type Response = http::Response<tonic::body::Body>;
        type Error = std::convert::Infallible;
        type Future = BoxFuture<Self::Response, Self::Error>;
        fn poll_ready(
            &mut self,
            _cx: &mut Context<'_>,
        ) -> Poll<std::result::Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, req: http::Request<B>) -> Self::Future {
            match req.uri().path() {
                "/dashboard.v1.DashboardService/GetAllHosts" => {
                    #[allow(non_camel_case_types)]
                    struct GetAllHostsSvc<T: DashboardService>(pub Arc<T>);
                    impl<
                        T: DashboardService,
                    > tonic::server::UnaryService<super::GetAllHostsRequest>
                    for GetAllHostsSvc<T> {
                        type Response = super::GetHostsResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::GetAllHostsRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::get_all_hosts(&inner, request)
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = GetAllHostsSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/dashboard.v1.DashboardService/UpdateHostName" => {
                    #[allow(non_camel_case_types)]
                    struct UpdateHostNameSvc<T: DashboardService>(pub Arc<T>);
                    impl<
                        T: DashboardService,
                    > tonic::server::UnaryService<super::UpdateNameRequest>
                    for UpdateHostNameSvc<T> {
                        type Response = super::Host;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::UpdateNameRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::update_host_name(&inner, request)
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = UpdateHostNameSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/dashboard.v1.DashboardService/DeleteHost" => {
                    #[allow(non_camel_case_types)]
                    struct DeleteHostSvc<T: DashboardService>(pub Arc<T>);
                    impl<T: DashboardService> tonic::server::UnaryService<super::Id>
                    for DeleteHostSvc<T> {
                        type Response = ();
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::Id>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::delete_host(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = DeleteHostSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/dashboard.v1.DashboardService/ConnectionState" => {
                    #[allow(non_camel_case_types)]
                    struct ConnectionStateSvc<T: DashboardService>(pub Arc<T>);
                    impl<T: DashboardService> tonic::server::ServerStreamingService<()>
                    for ConnectionStateSvc<T> {
                        type Response = super::HostConnectionEvent;
                        type ResponseStream = T::ConnectionStateStream;
                        type Future = BoxFuture<
                            tonic::Response<Self::ResponseStream>,
                            tonic::Status,
                        >;
                        fn call(&mut self, request: tonic::Request<()>) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::connection_state(&inner, request)
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = ConnectionStateSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.server_streaming(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/dashboard.v1.DashboardService/Deploy" => {
                    #[allow(non_camel_case_types)]
                    struct DeploySvc<T: DashboardService>(pub Arc<T>);
                    impl<
                        T: DashboardService,
                    > tonic::server::UnaryService<super::DeployHostRequest>
                    for DeploySvc<T> {
                        type Response = super::Task;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::DeployHostRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::deploy(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = DeploySvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/dashboard.v1.DashboardService/Multicast" => {
                    #[allow(non_camel_case_types)]
                    struct MulticastSvc<T: DashboardService>(pub Arc<T>);
                    impl<
                        T: DashboardService,
                    > tonic::server::UnaryService<super::MulticastHostsRequest>
                    for MulticastSvc<T> {
                        type Response = ();
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::MulticastHostsRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::multicast(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = MulticastSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/dashboard.v1.DashboardService/Reboot" => {
                    #[allow(non_camel_case_types)]
                    struct RebootSvc<T: DashboardService>(pub Arc<T>);
                    impl<
                        T: DashboardService,
                    > tonic::server::UnaryService<super::RebootHostsRequest>
                    for RebootSvc<T> {
                        type Response = ();
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::RebootHostsRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::reboot(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = RebootSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/dashboard.v1.DashboardService/WakeOnLan" => {
                    #[allow(non_camel_case_types)]
                    struct WakeOnLanSvc<T: DashboardService>(pub Arc<T>);
                    impl<
                        T: DashboardService,
                    > tonic::server::UnaryService<super::WakeOnLanRequest>
                    for WakeOnLanSvc<T> {
                        type Response = ();
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::WakeOnLanRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::wake_on_lan(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = WakeOnLanSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/dashboard.v1.DashboardService/GetAllImages" => {
                    #[allow(non_camel_case_types)]
                    struct GetAllImagesSvc<T: DashboardService>(pub Arc<T>);
                    impl<T: DashboardService> tonic::server::UnaryService<()>
                    for GetAllImagesSvc<T> {
                        type Response = super::GetImagesResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(&mut self, request: tonic::Request<()>) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::get_all_images(&inner, request)
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = GetAllImagesSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/dashboard.v1.DashboardService/UpdateImageName" => {
                    #[allow(non_camel_case_types)]
                    struct UpdateImageNameSvc<T: DashboardService>(pub Arc<T>);
                    impl<
                        T: DashboardService,
                    > tonic::server::UnaryService<super::UpdateNameRequest>
                    for UpdateImageNameSvc<T> {
                        type Response = super::Image;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::UpdateNameRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::update_image_name(&inner, request)
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = UpdateImageNameSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/dashboard.v1.DashboardService/CreateImage" => {
                    #[allow(non_camel_case_types)]
                    struct CreateImageSvc<T: DashboardService>(pub Arc<T>);
                    impl<
                        T: DashboardService,
                    > tonic::server::UnaryService<super::CreateImageRequest>
                    for CreateImageSvc<T> {
                        type Response = super::Image;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::CreateImageRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::create_image(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = CreateImageSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/dashboard.v1.DashboardService/DeleteImage" => {
                    #[allow(non_camel_case_types)]
                    struct DeleteImageSvc<T: DashboardService>(pub Arc<T>);
                    impl<T: DashboardService> tonic::server::UnaryService<super::Id>
                    for DeleteImageSvc<T> {
                        type Response = ();
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::Id>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::delete_image(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = DeleteImageSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/dashboard.v1.DashboardService/GetAllTasks" => {
                    #[allow(non_camel_case_types)]
                    struct GetAllTasksSvc<T: DashboardService>(pub Arc<T>);
                    impl<T: DashboardService> tonic::server::UnaryService<()>
                    for GetAllTasksSvc<T> {
                        type Response = super::GetTasksResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(&mut self, request: tonic::Request<()>) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::get_all_tasks(&inner, request)
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = GetAllTasksSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/dashboard.v1.DashboardService/CancelTask" => {
                    #[allow(non_camel_case_types)]
                    struct CancelTaskSvc<T: DashboardService>(pub Arc<T>);
                    impl<T: DashboardService> tonic::server::UnaryService<super::Id>
                    for CancelTaskSvc<T> {
                        type Response = ();
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::Id>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::cancel_task(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = CancelTaskSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/dashboard.v1.DashboardService/RetryTask" => {
                    #[allow(non_camel_case_types)]
                    struct RetryTaskSvc<T: DashboardService>(pub Arc<T>);
                    impl<T: DashboardService> tonic::server::UnaryService<super::Id>
                    for RetryTaskSvc<T> {
                        type Response = ();
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::Id>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::retry_task(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = RetryTaskSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/dashboard.v1.DashboardService/CreateGroup" => {
                    #[allow(non_camel_case_types)]
                    struct CreateGroupSvc<T: DashboardService>(pub Arc<T>);
                    impl<
                        T: DashboardService,
                    > tonic::server::UnaryService<super::CreateGroupRequest>
                    for CreateGroupSvc<T> {
                        type Response = super::Group;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::CreateGroupRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::create_group(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = CreateGroupSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/dashboard.v1.DashboardService/UpdateGroupName" => {
                    #[allow(non_camel_case_types)]
                    struct UpdateGroupNameSvc<T: DashboardService>(pub Arc<T>);
                    impl<
                        T: DashboardService,
                    > tonic::server::UnaryService<super::UpdateNameRequest>
                    for UpdateGroupNameSvc<T> {
                        type Response = super::Group;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::UpdateNameRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::update_group_name(&inner, request)
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = UpdateGroupNameSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/dashboard.v1.DashboardService/GetAllGroups" => {
                    #[allow(non_camel_case_types)]
                    struct GetAllGroupsSvc<T: DashboardService>(pub Arc<T>);
                    impl<T: DashboardService> tonic::server::UnaryService<()>
                    for GetAllGroupsSvc<T> {
                        type Response = super::GetGroupsResponse;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(&mut self, request: tonic::Request<()>) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::get_all_groups(&inner, request)
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = GetAllGroupsSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/dashboard.v1.DashboardService/UpdateGroupMemberships" => {
                    #[allow(non_camel_case_types)]
                    struct UpdateGroupMembershipsSvc<T: DashboardService>(pub Arc<T>);
                    impl<
                        T: DashboardService,
                    > tonic::server::UnaryService<super::UpdateGroupRequest>
                    for UpdateGroupMembershipsSvc<T> {
                        type Response = super::Group;
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::UpdateGroupRequest>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::update_group_memberships(
                                        &inner,
                                        request,
                                    )
                                    .await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = UpdateGroupMembershipsSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                "/dashboard.v1.DashboardService/DeleteGroup" => {
                    #[allow(non_camel_case_types)]
                    struct DeleteGroupSvc<T: DashboardService>(pub Arc<T>);
                    impl<T: DashboardService> tonic::server::UnaryService<super::Id>
                    for DeleteGroupSvc<T> {
                        type Response = ();
                        type Future = BoxFuture<
                            tonic::Response<Self::Response>,
                            tonic::Status,
                        >;
                        fn call(
                            &mut self,
                            request: tonic::Request<super::Id>,
                        ) -> Self::Future {
                            let inner = Arc::clone(&self.0);
                            let fut = async move {
                                <T as DashboardService>::delete_group(&inner, request).await
                            };
                            Box::pin(fut)
                        }
                    }
                    let accept_compression_encodings = self.accept_compression_encodings;
                    let send_compression_encodings = self.send_compression_encodings;
                    let max_decoding_message_size = self.max_decoding_message_size;
                    let max_encoding_message_size = self.max_encoding_message_size;
                    let inner = self.inner.clone();
                    let fut = async move {
                        let method = DeleteGroupSvc(inner);
                        let codec = tonic_prost::ProstCodec::default();
                        let mut grpc = tonic::server::Grpc::new(codec)
                            .apply_compression_config(
                                accept_compression_encodings,
                                send_compression_encodings,
                            )
                            .apply_max_message_size_config(
                                max_decoding_message_size,
                                max_encoding_message_size,
                            );
                        let res = grpc.unary(method, req).await;
                        Ok(res)
                    };
                    Box::pin(fut)
                }
                _ => {
                    Box::pin(async move {
                        let mut response = http::Response::new(
                            tonic::body::Body::default(),
                        );
                        let headers = response.headers_mut();
                        headers
                            .insert(
                                tonic::Status::GRPC_STATUS,
                                (tonic::Code::Unimplemented as i32).into(),
                            );
                        headers
                            .insert(
                                http::header::CONTENT_TYPE,
                                tonic::metadata::GRPC_CONTENT_TYPE,
                            );
                        Ok(response)
                    })
                }
            }
        }
    }
    impl<T> Clone for DashboardServiceServer<T> {
        fn clone(&self) -> Self {
            let inner = self.inner.clone();
            Self {
                inner,
                accept_compression_encodings: self.accept_compression_encodings,
                send_compression_encodings: self.send_compression_encodings,
                max_decoding_message_size: self.max_decoding_message_size,
                max_encoding_message_size: self.max_encoding_message_size,
            }
        }
    }
    /// Generated gRPC service name
    pub const SERVICE_NAME: &str = "dashboard.v1.DashboardService";
    impl<T> tonic::server::NamedService for DashboardServiceServer<T> {
        const NAME: &'static str = SERVICE_NAME;
    }
}
