{-# LANGUAGE ScopedTypeVariables #-}
import Transient.Base
import Transient.Internals
import Transient.Indeterminism
import Control.Monad
import Control.Applicative
import Data.Typeable
import Control.Monad.IO.Class
import Control.Concurrent
import Control.Exception hiding (onException, catch)
import qualified System.Timeout as T
import Data.IORef

say :: String -> TransIO ()
say = liftIO . putStrLn

run :: String -> TransIO () -> IO ()
run name t = do
  putStrLn ("--- " ++ name)
  r <- T.timeout 3000000 (keep' t)
  case r of
    Nothing -> putStrLn "   (keep' returned nothing in 3 s)"
    Just rs -> putStrLn ("   keep' returns " ++ show (length rs) ++ " results")

main = do
  run "B1 burst: one result and then silence" $ do
    n <- liftIO $ newIORef (0 :: Int)
    r <- burst 100000 $ async (do threadDelay 20000; atomicModifyIORef n (\a -> (a+1, a+1)))
    say ("burst " ++ show (r :: StreamData Int))
    case r of SDone -> exit () ; _ -> return ()

  run "B2 burst: three results in a row and then silence: a single SDone" $ do
    n <- liftIO $ newIORef (0 :: Int)
    r <- burst 100000 $ waitEvents (do threadDelay 30000
                                       k <- atomicModifyIORef n (\a -> (a+1, a+1))
                                       when (k > 3) $ threadDelay 100000000
                                       return k)
    say ("burst " ++ show (r :: StreamData Int))
    case r of SDone -> exit () ; _ -> return ()

  -- whileException (Exp16)
  run "W1 whileException retries until it succeeds" $ do
    n <- liftIO $ newIORef (0 :: Int)
    r <- whileException (do k <- liftIO (atomicModifyIORef n (\a -> (a+1, a+1)))
                            when (k < 3) $ liftIO (throwIO (ErrorCall (show k)))
                            return k)
                        (\(e :: ErrorCall) -> say ("  failed " ++ show e))
    say ("  ok after " ++ show r)
