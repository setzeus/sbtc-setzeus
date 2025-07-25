# \DepositApi

All URIs are relative to *http://localhost*

Method | HTTP request | Description
------------- | ------------- | -------------
[**create_deposit**](DepositApi.md#create_deposit) | **POST** /deposit | Create deposit handler.
[**get_deposit**](DepositApi.md#get_deposit) | **GET** /deposit/{txid}/{index} | Get deposit handler.
[**get_deposits**](DepositApi.md#get_deposits) | **GET** /deposit | Get deposits handler.
[**get_deposits_for_recipient**](DepositApi.md#get_deposits_for_recipient) | **GET** /deposit/recipient/{recipient} | Get deposits by recipient handler.
[**get_deposits_for_reclaim_pubkeys**](DepositApi.md#get_deposits_for_reclaim_pubkeys) | **GET** /deposit/reclaim-pubkeys/{reclaimPubkeys} | Get deposits by recipient handler.
[**get_deposits_for_transaction**](DepositApi.md#get_deposits_for_transaction) | **GET** /deposit/{txid} | Get deposits for transaction handler.
[**update_deposits_sidecar**](DepositApi.md#update_deposits_sidecar) | **PUT** /deposit_private | Update deposits handler.
[**update_deposits_signer**](DepositApi.md#update_deposits_signer) | **PUT** /deposit | Update deposits handler.



## create_deposit

> models::Deposit create_deposit(create_deposit_request_body)
Create deposit handler.

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**create_deposit_request_body** | [**CreateDepositRequestBody**](CreateDepositRequestBody.md) |  | [required] |

### Return type

[**models::Deposit**](Deposit.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)


## get_deposit

> models::Deposit get_deposit(txid, index)
Get deposit handler.

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**txid** | **String** | txid associated with the Deposit. | [required] |
**index** | **String** | output index associated with the Deposit. | [required] |

### Return type

[**models::Deposit**](Deposit.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)


## get_deposits

> models::GetDepositsResponse get_deposits(status, next_token, page_size)
Get deposits handler.

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**status** | [**DepositStatus**](.md) | the status to search by when getting all deposits. | [required] |
**next_token** | Option<**String**> | the next token value from the previous return of this api call. |  |
**page_size** | Option<**u32**> | the maximum number of items in the response list. |  |

### Return type

[**models::GetDepositsResponse**](GetDepositsResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)


## get_deposits_for_recipient

> models::GetDepositsResponse get_deposits_for_recipient(recipient, next_token, page_size)
Get deposits by recipient handler.

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**recipient** | **String** | the recipient to search by when getting all deposits. | [required] |
**next_token** | Option<**String**> | the next token value from the previous return of this api call. |  |
**page_size** | Option<**u32**> | the maximum number of items in the response list. |  |

### Return type

[**models::GetDepositsResponse**](GetDepositsResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)


## get_deposits_for_reclaim_pubkeys

> models::GetDepositsResponse get_deposits_for_reclaim_pubkeys(reclaim_pubkeys, next_token, page_size)
Get deposits by recipient handler.

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**reclaim_pubkeys** | **String** | The dash-separated list of hex-encoded x-only pubkeys used to generate the reclaim_script. | [required] |
**next_token** | Option<**String**> | the next token value from the previous return of this api call. |  |
**page_size** | Option<**u32**> | the maximum number of items in the response list. |  |

### Return type

[**models::GetDepositsResponse**](GetDepositsResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)


## get_deposits_for_transaction

> models::GetDepositsForTransactionResponse get_deposits_for_transaction(txid, next_token, page_size)
Get deposits for transaction handler.

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**txid** | **String** | txid associated with the Deposit. | [required] |
**next_token** | Option<**String**> | the next token value from the previous return of this api call. |  |
**page_size** | Option<**u32**> | the maximum number of items in the response list. |  |

### Return type

[**models::GetDepositsForTransactionResponse**](GetDepositsForTransactionResponse.md)

### Authorization

No authorization required

### HTTP request headers

- **Content-Type**: Not defined
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)


## update_deposits_sidecar

> models::UpdateDepositsResponse update_deposits_sidecar(update_deposits_request_body)
Update deposits handler.

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**update_deposits_request_body** | [**UpdateDepositsRequestBody**](UpdateDepositsRequestBody.md) |  | [required] |

### Return type

[**models::UpdateDepositsResponse**](UpdateDepositsResponse.md)

### Authorization

[ApiGatewayKey](../README.md#ApiGatewayKey)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)


## update_deposits_signer

> models::UpdateDepositsResponse update_deposits_signer(update_deposits_request_body)
Update deposits handler.

### Parameters


Name | Type | Description  | Required | Notes
------------- | ------------- | ------------- | ------------- | -------------
**update_deposits_request_body** | [**UpdateDepositsRequestBody**](UpdateDepositsRequestBody.md) |  | [required] |

### Return type

[**models::UpdateDepositsResponse**](UpdateDepositsResponse.md)

### Authorization

[ApiGatewayKey](../README.md#ApiGatewayKey)

### HTTP request headers

- **Content-Type**: application/json
- **Accept**: application/json

[[Back to top]](#) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to Model list]](../README.md#documentation-for-models) [[Back to README]](../README.md)

